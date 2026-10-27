"""Starts a real ag-server on a random loopback port with a throw-away data directory.

Hermetic: no network except 127.0.0.1; the process is killed (and the directory removed) on
``stop()``, and also when the test process exits for any reason.
"""

from __future__ import annotations

import atexit
import os
import re
import secrets
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import httpx

from agentgit import AgentGitClient, AsyncAgentGitClient

REPO_ROOT = Path(__file__).resolve().parents[3]
_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_running: set[RunningServer] = set()


def find_server_binary() -> str:
    candidates = [
        os.environ.get("AG_SERVER_BIN"),
        "/home/user/bin/ag-server",
        str(REPO_ROOT / "target/debug/ag-server"),
        str(REPO_ROOT / "target/release/ag-server"),
    ]
    for c in candidates:
        if c and Path(c).exists():
            return c
    raise RuntimeError(
        "ag-server binary not found (looked in: "
        + ", ".join(c for c in candidates if c)
        + "). Build it with `cargo build -p agentgit-server` or set AG_SERVER_BIN."
    )


@dataclass
class RunningServer:
    base_url: str
    bootstrap_token: str
    data_dir: Path
    process: subprocess.Popen[str]
    _log: list[str] = field(default_factory=list)

    def admin(self, **options: Any) -> AgentGitClient:
        """A client with the bootstrap administrator credential."""
        return AgentGitClient(self.base_url, self.bootstrap_token, **options)

    def anonymous(self, **options: Any) -> AgentGitClient:
        return AgentGitClient(self.base_url, **options)

    def actor(
        self,
        subject: str,
        repo: str,
        role: str,
        *,
        ttl_secs: int = 900,
        scope: dict[str, Any] | None = None,
        **options: Any,
    ) -> AgentGitClient:
        """Grants ``role`` on ``repo`` to ``subject`` and returns a client authenticating as ``subject``."""
        with self.admin() as admin:
            admin.roles.grant(subject=subject, repo=repo, role=role)
            minted = admin.credentials.mint(subject=subject, ttl_secs=ttl_secs, scope=scope)
        return AgentGitClient(self.base_url, minted.token, **options)

    def async_admin(self, **options: Any) -> AsyncAgentGitClient:
        return AsyncAgentGitClient(self.base_url, self.bootstrap_token, **options)

    def async_actor(self, subject: str, repo: str, role: str, **options: Any) -> AsyncAgentGitClient:
        """Like :meth:`actor`, for the asynchronous client (the credential is minted synchronously)."""
        with self.admin() as admin:
            admin.roles.grant(subject=subject, repo=repo, role=role)
            minted = admin.credentials.mint(subject=subject, ttl_secs=900)
        return AsyncAgentGitClient(self.base_url, minted.token, **options)

    def log(self) -> str:
        return "".join(self._log)

    def kill_now(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
        shutil.rmtree(self.data_dir, ignore_errors=True)

    def stop(self) -> None:
        _running.discard(self)
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        shutil.rmtree(self.data_dir, ignore_errors=True)

    def __hash__(self) -> int:
        return id(self)


@atexit.register
def _kill_all() -> None:
    for server in list(_running):
        server.kill_now()


def start_server(config: str | None = None) -> RunningServer:
    """Starts ``ag-server serve`` on 127.0.0.1:0 and waits until /healthz answers."""
    binary = find_server_binary()
    data_dir = Path(tempfile.mkdtemp(prefix="agentgit-sdk-py-"))
    token = secrets.token_hex(24)
    args = [binary, "serve", "--data-dir", str(data_dir / "data"), "--listen", "127.0.0.1:0"]
    if config:
        cfg = data_dir / "server.toml"
        cfg.write_text(config)
        args += ["--config", str(cfg)]
    # Address discovery consumes the INFO listening event, regardless of the caller's log filter.
    env = {
        **os.environ,
        "AG_BOOTSTRAP_TOKEN": token,
        "NO_COLOR": "1",
        "RUST_BACKTRACE": "0",
        "RUST_LOG": "info",
    }
    process = subprocess.Popen(
        args, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1
    )
    lines: list[str] = []
    found: list[str] = []
    ready = threading.Event()

    def pump() -> None:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line)
            m = re.search(r"(127\.0\.0\.1:\d+)", _ANSI.sub("", line))
            if m and "listening" in line and not found:
                found.append(m.group(1))
                ready.set()
        ready.set()

    threading.Thread(target=pump, daemon=True).start()
    ready.wait(timeout=20)
    if not found:
        process.kill()
        shutil.rmtree(data_dir, ignore_errors=True)
        raise RuntimeError("ag-server did not report its address:\n" + "".join(lines))
    server = RunningServer(f"http://{found[0]}", token, data_dir, process, lines)
    _running.add(server)
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            if httpx.get(f"{server.base_url}/healthz", timeout=1).status_code == 200:
                return server
        except httpx.HTTPError:
            time.sleep(0.05)
    server.stop()
    raise RuntimeError("ag-server never became healthy:\n" + "".join(lines))


ServerFactory = Callable[..., RunningServer]
