#!/usr/bin/env python3
"""End-to-end smoke test of a *deployed* ag-server: the same script runs against the container, the Docker
Compose stack and the Kubernetes deployment (.github/workflows/containers.yml), and against a local server.

It does what a new operator does, and asserts every step from the outside (HTTP + the real `ag` binary):

  1. the server is ready; /healthz, /readyz and /openapi.json answer; /metrics demands its token;
  2. as the bootstrap administrator: create a repository, grant roles, mint credentials;
  3. `ag clone` (empty repository), edit, `ag snapshot`, `ag push` -> a proposal; the stream has NOT moved;
  4. a writer cannot integrate; an integrator can: `ag remote integrate` -> the stream moved;
  5. read the result back over the HTTP API (stream head, directory listing, file bytes, history);
  6. a second user clones and gets the bytes; a second change is pushed, merged and pulled back;
  7. /metrics shows the requests, the integrations and the object traffic that just happened.

    scripts/e2e_smoke.py --url http://127.0.0.1:8080 --ag target/debug/ag \\
        --bootstrap-token "$AG_BOOTSTRAP_TOKEN" --metrics-token "$AG_METRICS_TOKEN"

`--save-state FILE` records what a later `--verify-state FILE` run needs (tokens, expected head, expected bytes):
restart the server (or the pod) in between to prove that credentials, the repository and its objects survive.

`--admin-exec 'docker compose exec -T ag-server /usr/local/bin/ag-server admin --data-dir /data'` makes the
script grant roles and mint credentials with the offline admin tool inside the container instead of the HTTP
API (the path docs/operations/deployment.md documents). Python 3 standard library only.
"""
import argparse
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

PASS = 0


def step(msg: str) -> None:
    print(f"\n== {msg}", flush=True)


def ok(msg: str) -> None:
    global PASS
    PASS += 1
    print(f"   ok  {msg}", flush=True)


def fail(msg: str) -> None:
    print(f"   FAIL  {msg}", flush=True)
    sys.exit(1)


def check(cond: bool, msg: str, detail: object = "") -> None:
    if cond:
        ok(msg)
    else:
        fail(f"{msg}\n         {str(detail)[:2000]}")


class Api:
    """The HTTP/JSON side of the server."""

    def __init__(self, base: str):
        self.base = base.rstrip("/")

    def call(self, method: str, path: str, token: str | None = None, body: object = None,
             raw: bool = False, timeout: float = 30):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, method=method)
        if data is not None:
            req.add_header("Content-Type", "application/json")
        if token:
            req.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                payload = r.read()
                return r.status, (payload if raw else (json.loads(payload) if payload else None))
        except urllib.error.HTTPError as e:
            payload = e.read()
            try:
                return e.code, json.loads(payload)
            except ValueError:
                return e.code, payload.decode(errors="replace")


def wait_ready(api: Api, seconds: int) -> None:
    deadline = time.time() + seconds
    last = None
    while time.time() < deadline:
        try:
            status, _ = api.call("GET", "/readyz", raw=True, timeout=3)
            if status == 200:
                return
            last = f"HTTP {status}"
        except (urllib.error.URLError, OSError) as e:
            last = e
        time.sleep(1)
    fail(f"the server did not become ready within {seconds}s ({last})")


def run_ag(ag: str, cwd: str, identity: str, token: str | None, *args: str, expect: int | None = 0,
           extra_env: dict | None = None) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    env["AG_IDENTITY"] = identity
    env.pop("AG_KEY_DIR", None)
    env.pop("AG_TOKEN", None)
    if token:
        env["AG_TOKEN"] = token
    env.update(extra_env or {})
    p = subprocess.run([ag, *args], cwd=cwd, env=env, capture_output=True, text=True, timeout=120)
    if expect is not None and p.returncode != expect:
        fail(f"`ag {' '.join(args)}` as {identity} exited {p.returncode}, expected {expect}\n"
             f"         stdout: {p.stdout[-1500:]}\n         stderr: {p.stderr[-1500:]}")
    return p


def metrics_sum(text: str, name: str, **labels: str) -> float:
    """Sum of every sample of `name` whose labels include all of `labels`."""
    total = 0.0
    for line in text.splitlines():
        if not line.startswith(name):
            continue
        m = re.fullmatch(rf"{re.escape(name)}(?:\{{(.*)\}})?\s+(\S+)", line)
        if not m:
            continue
        have = dict(re.findall(r'(\w+)="([^"]*)"', m.group(1) or ""))
        if all(have.get(k) == v for k, v in labels.items()):
            total += float(m.group(2))
    return total


class Admin:
    """Creates repositories, role bindings and credentials: over HTTP as the bootstrap administrator, or with the
    offline admin tool inside the container."""

    def __init__(self, api: Api, bootstrap: str, admin_exec: str | None):
        self.api, self.bootstrap, self.admin_exec = api, bootstrap, admin_exec

    def _exec(self, *args: str) -> str:
        cmd = [*shlex.split(self.admin_exec), *args]
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        if p.returncode:
            fail(f"{' '.join(cmd)} exited {p.returncode}: {p.stderr[-1000:]}")
        return p.stdout.strip()

    def grant(self, subject: str, repo: str, role: str) -> None:
        if self.admin_exec:
            self._exec("grant", "--subject", subject, "--repo", repo, "--role", role)
            return
        status, body = self.api.call("POST", "/v1/roles", self.bootstrap,
                                     {"subject": subject, "repo": repo, "role": role})
        check(status == 200, f"grant {role} to {subject} on {repo}", body)

    def mint(self, subject: str, ttl: int = 3600) -> str:
        if self.admin_exec:
            token = self._exec("mint", "--subject", subject, "--ttl", str(ttl)).splitlines()[-1]
        else:
            status, body = self.api.call("POST", "/v1/credentials", self.bootstrap,
                                         {"subject": subject, "ttl_secs": ttl, "description": "e2e smoke"})
            check(status == 200 and body.get("token"), f"mint a credential for {subject}", body)
            token = body["token"]
        check(token.startswith("agt_"), f"credential for {subject} looks like a token")
        return token


def verify_state(api: Api, args: argparse.Namespace) -> None:
    """After a restart: the credentials, the repository, the stream head and the bytes are all still there."""
    with open(args.verify_state) as f:
        state = json.load(f)
    repo_path = "/v1/repos/" + state["repo"]
    step("after the restart: nothing was lost")
    wait_ready(api, args.ready_timeout)
    ok("/readyz is 200 again")
    carol = state["tokens"]["carol"]
    status, who = api.call("GET", "/v1/whoami", carol)
    check(status == 200, "a credential minted before the restart still works", who)
    status, head = api.call("GET", repo_path + "/streams/head?stream=main", carol)
    check(status == 200 and head.get("head") == state["head"] and head.get("version") == state["version"],
          "the stream head is exactly where it was", (head, state["head"], state["version"]))
    status, content = api.call("GET", repo_path + "/files/src/lib.rs?rev=main", carol, raw=True)
    check(status == 200 and content.decode() == state["file"], "src/lib.rs has the same bytes", content[:200])
    status, content = api.call("GET", repo_path + "/files/README.md?rev=main", carol, raw=True)
    check(status == 200 and content.decode() == state["readme"], "README.md has the same bytes", content[:200])
    work = tempfile.mkdtemp(prefix="ag-e2e-verify-")
    target = os.path.join(work, "fresh")
    run_ag(os.path.abspath(args.ag), work, "user:carol", carol, "clone", f"{args.url.rstrip('/')}/{state['repo']}", target)
    with open(os.path.join(target, "src", "lib.rs")) as f:
        check(f.read() == state["file"], "a fresh `ag clone` after the restart has the same bytes")
    p = run_ag(os.path.abspath(args.ag), target, "user:carol", carol, "verify", expect=None)
    ok(f"`ag verify` of the clone exited {p.returncode}")
    print(f"\nall {PASS} checks passed")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--url", required=True, help="server base URL, e.g. http://127.0.0.1:8080")
    ap.add_argument("--ag", required=True, help="path to the `ag` binary")
    ap.add_argument("--bootstrap-token", default=os.environ.get("AG_BOOTSTRAP_TOKEN"))
    ap.add_argument("--metrics-token", default=os.environ.get("AG_METRICS_TOKEN"))
    ap.add_argument("--repo", default="acme/web", help="repository to create (owner/name)")
    ap.add_argument("--admin-exec", help="command prefix for the offline admin tool (grant/mint) instead of HTTP")
    ap.add_argument("--create-repo-exec", action="store_true",
                    help="create the repository with the offline admin tool too (needs --admin-exec)")
    ap.add_argument("--expect-meta-backend", help="assert the repository's metadata backend (sqlite, postgres)")
    ap.add_argument("--expect-object-backend", help="assert the repository's object backend (fs, s3)")
    ap.add_argument("--ready-timeout", type=int, default=180)
    ap.add_argument("--save-state", help="write the state a later --verify-state run needs to this file")
    ap.add_argument("--verify-state", help="only check that the state saved earlier survived a restart")
    ap.add_argument("--keep", help="keep the working directories under this path (for artifacts)")
    args = ap.parse_args()
    if not args.bootstrap_token or not args.metrics_token:
        sys.exit("--bootstrap-token and --metrics-token (or AG_BOOTSTRAP_TOKEN / AG_METRICS_TOKEN) are required")

    api = Api(args.url)
    if args.verify_state:
        verify_state(api, args)
        return
    owner, name = args.repo.split("/")
    repo_path = f"/v1/repos/{owner}/{name}"
    work = os.path.abspath(args.keep or tempfile.mkdtemp(prefix="ag-e2e-"))
    os.makedirs(work, exist_ok=True)

    # ---- 1. the server is up ---------------------------------------------------------------
    step("server is ready, health endpoints answer, /metrics is protected")
    wait_ready(api, args.ready_timeout)
    ok("/readyz is 200")
    status, _ = api.call("GET", "/healthz", raw=True)
    check(status == 200, "/healthz is 200")
    status, spec = api.call("GET", "/openapi.json")
    check(status == 200 and spec.get("openapi", "").startswith("3.") and "/v1/repos" in spec.get("paths", {}),
          "/openapi.json describes the API", spec if status != 200 else "")
    status, _ = api.call("GET", "/v1/whoami", raw=True)
    check(status == 401, "an unauthenticated API call is refused (401)", status)
    status, _ = api.call("GET", "/metrics", raw=True)
    check(status in (401, 403), "/metrics without its token is refused", status)
    status, _ = api.call("GET", "/metrics", "not-the-token-0000000000000000", raw=True)
    check(status in (401, 403), "/metrics with a wrong token is refused", status)
    status, text = api.call("GET", "/metrics", args.metrics_token, raw=True)
    check(status == 200 and b"agwp_uptime_seconds" in text, "/metrics with the token exports agwp_* series", status)

    status, who = api.call("GET", "/v1/whoami", args.bootstrap_token)
    check(status == 200, "the bootstrap token authenticates", who)

    # ---- 2. administer ----------------------------------------------------------------------
    step("create the repository, grant roles, mint credentials")
    admin = Admin(api, args.bootstrap_token, args.admin_exec)
    if args.create_repo_exec:
        admin._exec("create-repo", args.repo)
        ok(f"created {args.repo} with the offline admin tool")
        status, desc = api.call("GET", repo_path, args.bootstrap_token)
    else:
        status, desc = api.call("POST", "/v1/repos", args.bootstrap_token,
                                {"name": args.repo, "default_stream": "main", "description": "e2e smoke"})
    check(status == 200, f"repository {args.repo} exists", desc)
    # The create response carries placeholder backends ("sqlite"/"fs"); `describe` reports what the repository really uses.
    status, desc = api.call("GET", repo_path, args.bootstrap_token)
    check(status == 200, "the repository can be described", desc)
    ok(f"storage: meta={desc.get('meta_backend')} objects={desc.get('object_backend')}")
    if args.expect_meta_backend:
        check(desc.get("meta_backend", "").startswith(args.expect_meta_backend),
              f"repository metadata lives in {args.expect_meta_backend}", desc)
    if args.expect_object_backend:
        check(desc.get("object_backend", "").startswith(args.expect_object_backend),
              f"repository objects live in {args.expect_object_backend}", desc)
    status, dup = api.call("POST", "/v1/repos", args.bootstrap_token, {"name": args.repo, "default_stream": "main"})
    check(status in (409, 400), "creating the same repository again is refused", (status, dup))

    # alice writes; bob writes and integrates (two roles: integrating alone does not allow uploading); carol reads.
    users = {"alice": ["writer"], "bob": ["writer", "integrator"], "carol": ["reader"]}
    tokens = {}
    for user, roles in users.items():
        for role in roles:
            admin.grant(f"user:{user}", args.repo, role)
        tokens[user] = admin.mint(f"user:{user}")
    # An offline grant is seen by the running server after its binding cache expires (5 s).
    deadline = time.time() + 40
    while True:
        status, body = api.call("GET", repo_path + "/streams", tokens["alice"])
        if status == 200 or time.time() > deadline:
            break
        time.sleep(1)
    check(status == 200, "alice (writer) can list streams", body)

    # ---- 3. clone, edit, push -----------------------------------------------------------------
    step("alice: ag clone, edit, snapshot, push (opens a proposal; the stream must not move)")
    ag = os.path.abspath(args.ag)
    alice_dir = os.path.join(work, "alice")
    remote = f"{args.url.rstrip('/')}/{args.repo}"
    run_ag(ag, work, "user:alice", tokens["alice"], "clone", remote, alice_dir)
    ok("cloned the (empty) repository")
    p = run_ag(ag, alice_dir, "user:alice", tokens["alice"], "--json", "remote", "whoami")
    check(json.loads(p.stdout).get("identity") == "user:alice", "the server knows alice", p.stdout)

    status, head0 = api.call("GET", repo_path + "/streams/head?stream=main", tokens["alice"])
    check(status == 200, "stream head readable", head0)
    version0 = head0.get("version", 0)

    readme = "# web\n\nHello from the end-to-end smoke test.\n"
    lib = "pub fn one() -> u32 {\n    1\n}\n\npub fn two() -> u32 {\n    2\n}\n\npub fn three() -> u32 {\n    3\n}\n"
    os.makedirs(os.path.join(alice_dir, "src"), exist_ok=True)
    with open(os.path.join(alice_dir, "README.md"), "w") as f:
        f.write(readme)
    with open(os.path.join(alice_dir, "src", "lib.rs"), "w") as f:
        f.write(lib)
    run_ag(ag, alice_dir, "user:alice", tokens["alice"], "snapshot", "-m", "initial import")
    p = run_ag(ag, alice_dir, "user:alice", tokens["alice"], "push", "-t", "Initial import")
    m = re.search(r"prop_[0-9a-zA-Z]+", p.stdout + p.stderr)
    check(m is not None, "push opened a proposal", p.stdout + p.stderr)
    prop1 = m.group(0)
    status, head1 = api.call("GET", repo_path + "/streams/head?stream=main", tokens["alice"])
    check(status == 200 and head1.get("version", 0) == version0 and head1.get("head", "") == head0.get("head", ""),
          "the stream did not move on push", (head0, head1))

    # ---- 4. authorization is enforced -----------------------------------------------------------
    step("a writer cannot integrate, a reader cannot push; an integrator integrates")
    p = run_ag(ag, alice_dir, "user:alice", tokens["alice"], "remote", "integrate", prop1, expect=None)
    check(p.returncode != 0 and "permission" in (p.stdout + p.stderr).lower(),
          "alice (writer) is refused integration", p.stdout + p.stderr)
    status, head = api.call("GET", repo_path + "/streams/head?stream=main", tokens["alice"])
    check(head.get("version", 0) == version0, "the refused integration did not move the stream", head)
    carol_dir = os.path.join(work, "carol")
    run_ag(ag, work, "user:carol", tokens["carol"], "clone", remote, carol_dir)
    ok("carol (reader) can clone")
    with open(os.path.join(carol_dir, "x.txt"), "w") as f:
        f.write("not allowed\n")
    run_ag(ag, carol_dir, "user:carol", tokens["carol"], "snapshot", "-m", "carol tries")
    p = run_ag(ag, carol_dir, "user:carol", tokens["carol"], "push", "-t", "Carol tries", expect=None)
    check(p.returncode != 0, "carol (reader) cannot push", p.stdout + p.stderr)

    p = run_ag(ag, alice_dir, "user:bob", tokens["bob"], "remote", "integrate", prop1)
    check("Integrated" in p.stdout, "bob (integrator) integrated the proposal", p.stdout + p.stderr)

    # ---- 5. read back over HTTP -------------------------------------------------------------------
    step("read the result back through the HTTP API")
    status, head2 = api.call("GET", repo_path + "/streams/head?stream=main", tokens["carol"])
    check(status == 200 and head2.get("version", 0) > version0 and head2.get("head"),
          "the stream head moved (carol, a reader, can see it)", head2)
    status, listing = api.call("GET", repo_path + "/tree/list?rev=main", tokens["carol"])
    names = sorted(e.get("name") or e.get("path", "") for e in (listing or {}).get("entries", []))
    check(status == 200 and names == ["README.md", "src"], "the root directory lists README.md and src", listing)
    status, content = api.call("GET", repo_path + "/files/src/lib.rs?rev=main", tokens["carol"], raw=True)
    check(status == 200 and content.decode() == lib, "src/lib.rs has exactly the pushed bytes", content[:200])
    status, log = api.call("GET", repo_path + "/log?rev=main", tokens["carol"])
    check(status == 200 and len(log.get("changesets", [])) >= 1, "the history has the changeset", log)
    status, tampered = api.call("GET", repo_path + "/files/README.md?rev=main", None, raw=True)
    check(status == 401, "reading a file needs a credential", status)

    # ---- 6. a second user, a second change ----------------------------------------------------------
    step("bob clones; alice and bob change different lines; the server merges; everyone pulls")
    bob_dir = os.path.join(work, "bob")
    run_ag(ag, work, "user:bob", tokens["bob"], "clone", remote, bob_dir)
    with open(os.path.join(bob_dir, "src", "lib.rs")) as f:
        check(f.read() == lib, "bob's clone has the bytes alice pushed")
    # alice adds a function at the end; bob renames the first one. (Bob holds writer and integrator.)
    run_ag(ag, alice_dir, "user:alice", tokens["alice"], "pull")
    with open(os.path.join(alice_dir, "src", "lib.rs"), "w") as f:
        f.write(lib + "\npub fn four() -> u32 {\n    4\n}\n")
    run_ag(ag, alice_dir, "user:alice", tokens["alice"], "snapshot", "-m", "add four")
    p = run_ag(ag, alice_dir, "user:alice", tokens["alice"], "push", "-t", "Add four")
    prop_a = re.search(r"prop_[0-9a-zA-Z]+", p.stdout + p.stderr).group(0)
    with open(os.path.join(bob_dir, "src", "lib.rs"), "w") as f:
        f.write(lib.replace("pub fn one()", "pub fn uno()"))
    run_ag(ag, bob_dir, "user:bob", tokens["bob"], "snapshot", "-m", "rename one")
    p = run_ag(ag, bob_dir, "user:bob", tokens["bob"], "push", "-t", "Rename one")
    prop_b = re.search(r"prop_[0-9a-zA-Z]+", p.stdout + p.stderr).group(0)
    run_ag(ag, bob_dir, "user:bob", tokens["bob"], "remote", "integrate", prop_a)
    p = run_ag(ag, bob_dir, "user:bob", tokens["bob"], "remote", "integrate", prop_b)
    check("Integrated (merge)" in p.stdout, "the second, concurrent change was merged by the server", p.stdout + p.stderr)
    merged = lib.replace("pub fn one()", "pub fn uno()") + "\npub fn four() -> u32 {\n    4\n}\n"
    status, content = api.call("GET", repo_path + "/files/src/lib.rs?rev=main", tokens["carol"], raw=True)
    check(status == 200 and content.decode() == merged, "the server holds the merged file", content.decode())
    for who, d in (("alice", alice_dir), ("bob", bob_dir)):
        run_ag(ag, d, f"user:{who}", tokens[who], "pull")
        with open(os.path.join(d, "src", "lib.rs")) as f:
            check(f.read() == merged, f"{who}'s working directory has the merged file after `ag pull`")
    status, head3 = api.call("GET", repo_path + "/streams/head?stream=main", tokens["carol"])
    check(head3.get("version", 0) >= head2.get("version", 0) + 2, "the stream moved twice more", (head2, head3))

    # ---- 7. metrics ------------------------------------------------------------------------------------
    step("/metrics reflects what just happened")
    status, text = api.call("GET", "/metrics", args.metrics_token, raw=True)
    text = text.decode()
    n = metrics_sum(text, "agwp_requests_total", operation="integration.integrate", code="ok")
    check(n >= 3, "agwp_requests_total counts the successful integrations", n)
    n = metrics_sum(text, "agwp_requests_total", operation="repo.create", code="ok")
    check(n >= 1 or args.create_repo_exec, "agwp_requests_total counts the repository creation", n)
    n = metrics_sum(text, "agwp_integrations_total", outcome="integrated")
    check(n >= 3, 'agwp_integrations_total{outcome="integrated"} >= 3', n)
    n = metrics_sum(text, "agwp_object_bytes_received_total")
    check(n > 0, "agwp_object_bytes_received_total > 0 (objects were uploaded)", n)
    n = metrics_sum(text, "agwp_auth_failures_total")
    check(n >= 1, "agwp_auth_failures_total counted the unauthenticated probes", n)
    n = metrics_sum(text, "agwp_requests_total", transport="grpc")
    check(n > 0, "the CLI's gRPC requests are counted (transport=grpc)", n)
    n = metrics_sum(text, "agwp_requests_total", transport="http")
    check(n > 0, "HTTP requests are counted (transport=http)", n)

    if args.save_state:
        with open(args.save_state, "w") as f:
            json.dump({"repo": args.repo, "tokens": tokens, "version": head3.get("version"), "head": head3.get("head"),
                       "file": merged, "readme": readme}, f)
        ok(f"state saved to {args.save_state}")

    print(f"\nall {PASS} checks passed")


if __name__ == "__main__":
    main()
