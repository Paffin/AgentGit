"""A real AgentGit server for the examples in this directory: starts `ag-server`, provisions it over its HTTP API
and tears it down. Python 3 standard library only.

Nothing here is simulated: `ag-server` is the production binary on a temporary data directory, and everything is
done through the same API a person or CI job would use, as the bootstrap administrator.
"""

from __future__ import annotations

import base64
import json
import os
import pathlib
import secrets
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

# The public key of the first test vector of RFC 8032 (Ed25519). The server wants *a* valid public key for an
# agent; the MCP server never signs anything, so nobody needs the matching secret key.
AGENT_PUBLIC_KEY = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"


def _cargo_target_dir() -> str | None:
    try:
        out = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], capture_output=True,
                             text=True, timeout=60)
        return json.loads(out.stdout)["target_directory"] if out.returncode == 0 else None
    except (OSError, ValueError, KeyError, subprocess.TimeoutExpired):
        return None


def find_binary(name: str, explicit: str | None = None) -> str:
    """The binary `name`: an explicit path, `AG_SERVER_BIN` / `AG_MCP_BIN`, a cargo target directory or the PATH."""
    if explicit:
        return explicit
    var = name.upper().replace("-", "_") + "_BIN"
    if os.environ.get(var):
        return os.environ[var]
    for root in (os.environ.get("CARGO_TARGET_DIR"), "target", _cargo_target_dir()):
        for profile in ("release", "debug"):
            if root and (pathlib.Path(root) / profile / name).is_file():
                return str(pathlib.Path(root) / profile / name)
    found = shutil.which(name)
    if found:
        return found
    raise SystemExit(f"cannot find `{name}`: build it (cargo build -p {name}) or set {var}")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Http:
    def __init__(self, base: str, token: str):
        self.base, self.token = base.rstrip("/"), token

    def call(self, method: str, path: str, body: object = None) -> tuple[int, object]:
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("Authorization", f"Bearer {self.token}")
        if data is not None:
            req.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                raw = r.read()
                return r.status, (json.loads(raw) if raw else None)
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                return e.code, json.loads(raw)
            except ValueError:
                return e.code, raw.decode(errors="replace")

    def ok(self, method: str, path: str, body: object = None) -> object:
        status, payload = self.call(method, path, body)
        if status != 200:
            raise RuntimeError(f"{method} {path} -> {status}: {payload}")
        return payload


class Stack:
    """A running `ag-server` with one repository, its seed content, a task and an agent with a credential."""

    def __init__(self, ag_server: str | None = None):
        self.ag_server = find_binary("ag-server", ag_server)
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix="agentgit-example-"))
        self.admin_token = secrets.token_hex(32)
        self.port = free_port()
        self.url = f"http://127.0.0.1:{self.port}"
        self.proc: subprocess.Popen | None = None
        self.log_path = self.dir / "server.log"

    def __enter__(self) -> "Stack":
        env = dict(os.environ, AG_BOOTSTRAP_TOKEN=self.admin_token)
        log = open(self.log_path, "wb")
        self.proc = subprocess.Popen(
            [self.ag_server, "serve", "--listen", f"127.0.0.1:{self.port}", "--data-dir", str(self.dir / "data")],
            env=env, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.time() + 60
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise RuntimeError("ag-server exited:\n" + self.log_path.read_text(errors="replace")[-2000:])
            try:
                with urllib.request.urlopen(self.url + "/readyz", timeout=2) as r:
                    if r.status == 200:
                        return self
            except (urllib.error.URLError, OSError):
                pass
            time.sleep(0.2)
        raise RuntimeError("ag-server did not become ready")

    def __exit__(self, *exc) -> None:
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.proc.kill()
        shutil.rmtree(self.dir, ignore_errors=True)

    # ---- provisioning ---------------------------------------------------------------------------------------

    @property
    def admin(self) -> Http:
        return Http(self.url, self.admin_token)

    def credential(self, subject: str, repo: str, role: str, ttl_secs: int = 3600) -> str:
        self.admin.ok("POST", "/v1/roles", {"subject": subject, "repo": repo, "role": role})
        out = self.admin.ok("POST", "/v1/credentials",
                            {"subject": subject, "ttl_secs": ttl_secs, "scope": {"repos": [repo]}})
        return out["token"]

    def create_repo(self, repo: str, files: dict[str, str], message: str = "Initial import") -> None:
        """Creates `repo` and lands `files` on its default stream the way a person would: workspace, proposal,
        integrate."""
        self.admin.ok("POST", "/v1/repos", {"name": repo})
        self.admin.ok("POST", "/v1/roles", {"subject": "user:seeder", "repo": repo, "role": "integrator"})
        h = Http(self.url, self.credential("user:seeder", repo, "writer"))
        ws = "seed"
        h.ok("POST", f"/v1/repos/{repo}/workspaces", {"name": ws, "stream": "main"})
        changes = [{"path": p, "op": "put", "content": base64.b64encode(c.encode()).decode()}
                   for p, c in files.items()]
        head = h.ok("POST", f"/v1/repos/{repo}/workspaces/{ws}/changes",
                    {"message": message, "changes": changes})["head"]
        prop = h.ok("POST", f"/v1/repos/{repo}/proposals", {"stream": "main", "head": head, "title": message})
        out = h.ok("POST", f"/v1/repos/{repo}/proposals/{prop['id']}/integrate", {})
        if out["outcome"]["status"] != "integrated":
            raise RuntimeError(f"seeding did not integrate: {out}")

    def create_task(self, repo: str, title: str, description: str, scope: list[str]) -> str:
        planner = Http(self.url, self.credential("user:planner", repo, "writer"))
        return planner.ok("POST", f"/v1/repos/{repo}/tasks",
                          {"title": title, "description": description, "scope": scope})["id"]

    def register_agent(self, repo: str, name: str) -> str:
        """Registers `agent:<name>` and returns a credential for it (writer: it can propose, not integrate)."""
        token = self.credential(f"agent:{name}", repo, "writer")
        Http(self.url, token).ok("POST", f"/v1/repos/{repo}/agents", {"name": name, "public_key": AGENT_PUBLIC_KEY})
        return token
