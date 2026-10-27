#!/usr/bin/env python3
"""A scripted agent (no model) that drives the real `ag-mcp` against a real `ag-server`.

It does what a coding agent does with the MCP tools - claim a task, take a lease, read, edit in a workspace, open
a proposal - and checks every step from the outside, so it doubles as an end-to-end test of the whole path
(CI runs it: the `mcp` job in .github/workflows/ci.yml). It also tries the things the operator's allow-list and
the server's policy must refuse, and a person then reviews and lands the proposal.

    examples/agents/mcp_scripted_agent.py [--ag-server PATH] [--ag-mcp PATH]

Exit status 0 only if every check passed. Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import os
import sys

from agentgit_stack import Http, Stack, find_binary
from mcp_client import McpClient

REPO = "acme/web"
OLD = "pub fn login(user: &str) -> bool {\n    !user.is_empty()\n}\n"
NEW = "pub fn login(user: &str) -> bool {\n    !user.trim().is_empty()\n}\n"
PASSED = 0


def check(cond: bool, what: str, detail: object = "") -> None:
    global PASSED
    if not cond:
        print(f"FAIL  {what}\n      {str(detail)[:1500]}", flush=True)
        sys.exit(1)
    PASSED += 1
    print(f"  ok  {what}", flush=True)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ag-server")
    ap.add_argument("--ag-mcp")
    args = ap.parse_args()
    ag_mcp = find_binary("ag-mcp", args.ag_mcp)

    with Stack(args.ag_server) as stack:
        stack.create_repo(REPO, {"README.md": "# Web\n", "src/lib.rs": "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
                                 "src/auth/login.rs": OLD, "docs/guide.md": "Guide\n"})
        task = stack.create_task(REPO, "Reject blank user names", 'login() accepts "  "; it should not.', ["src/auth/**"])
        token = stack.register_agent(REPO, "coder")
        print(f"server {stack.url}: repository {REPO}, task {task}, agent coder")

        # The operator's side: the narrowest configuration that lets the agent do this job.
        cmd = [ag_mcp, "--server", stack.url, "--repo", REPO, "--write-path", "src/**", "--write-path", "docs/**",
               "--require-lease"]
        env = dict(os.environ, AG_MCP_TOKEN=token)
        with McpClient(cmd, env) as mcp:
            d = mcp.discover()
            check(d["supportedVersions"][0] == "2026-07-28", "server/discover names the newest revision", d)
            names = [t["name"] for t in mcp.list_tools()]
            check("task_claim" in names and "propose" in names, "the work tools are offered")
            check("integrate" not in names and "review" not in names, "integrate and review are not offered", names)

            who = mcp.call("whoami")
            check(who.data["identity"] == "agent:coder", "whoami: the credential is the agent's", who.text)

            tasks = mcp.call("task_list", {"ready_only": True})
            check(tasks.data["tasks"][0]["id"] == task, "the task is ready", tasks.text)
            claim = mcp.call("task_claim", {"task_id": task})
            check(not claim.is_error, "task_claim starts a run", claim.text)
            run = claim.data["run"]["run_id"]
            check(claim.data["run"]["identity"] == f"agent:coder#{run}", "the run has an identity of its own")

            again = mcp.call("task_claim", {"task_id": task})
            check(again.is_error and run in again.text, "claiming the task again is refused and names the run", again.text)

            lease = mcp.call("lease_acquire", {"scope": "src/auth/**", "exclusive": True, "run_id": run, "task_id": task})
            check(not lease.is_error and lease.data["lease"]["fencing_token"] >= 1, "an exclusive lease with a fencing token", lease.text)
            lease_id = lease.data["lease"]["id"]

            before = mcp.call("read_file", {"path": "src/auth/login.rs"})
            check(before.data["content"] == OLD, "read_file returns the file as it is on main", before.text)
            ws = mcp.call("workspace_create", {"name": "fix-login", "run_id": run})
            check(not ws.is_error, "workspace_create", ws.text)

            # Refusals come from the operator's allow-list, before anything reaches the server.
            bad = mcp.call("workspace_write_files", {"workspace": "fix-login", "message": "x", "run_id": run,
                                                     "changes": [{"path": "Cargo.toml", "op": "put", "content": "x"}]})
            check(bad.is_error and bad.error_code == "path_not_allowed", "a write outside --write-path is refused", bad.text)
            bad = mcp.call("workspace_write_files", {"workspace": "fix-login", "message": "x", "run_id": run,
                                                     "changes": [{"path": "src/lib.rs", "op": "put", "content": "x"}]})
            check(bad.is_error and bad.error_code == "path_outside_lease", "a write outside the lease is refused", bad.text)

            snap = mcp.call("workspace_write_files", {
                "workspace": "fix-login", "message": "login: reject blank user names", "run_id": run,
                "lease_id": lease_id, "changes": [{"path": "src/auth/login.rs", "op": "put", "content": NEW}]})
            check(not snap.is_error, "the write inside the lease works", snap.text)
            edited = mcp.call("read_file", {"path": "src/auth/login.rs", "rev": "@fix-login"})
            check(edited.data["content"] == NEW, "the workspace holds the edit")
            main_file = mcp.call("read_file", {"path": "src/auth/login.rs"})
            check(main_file.data["content"] == OLD, "main is untouched")

            prop = mcp.call("propose", {"workspace": "fix-login", "title": "Reject blank user names",
                                        "description": "Trim before checking.", "task_id": task, "run_id": run})
            check(not prop.is_error, "propose opens a proposal", prop.text)
            pid = prop.data["proposal"]["id"]
            v = prop.data["status"]["verdicts"]
            check(v["mergeable"]["state"] == "ok" and v["approved"]["state"] == "blocked" and v["integrated"]["state"] == "pending",
                  "the four verdicts come back with the proposal", v)

            status = mcp.call("proposal_status", {"proposal_id": pid})
            check(status.data["ready_to_integrate"] is False and status.data["blockers"], "the blockers are explained", status.text)
            nope = mcp.call("integrate", {"proposal_id": pid})
            check(nope.is_error and nope.error_code == "tool_disabled", "calling integrate anyway is refused", nope.text)

            # A person reviews and lands it; the agent sees the outcome.
            rita = Http(stack.url, stack.credential("user:rita", REPO, "reviewer"))
            rita.ok("POST", f"/v1/repos/{REPO}/proposals/{pid}/reviews", {"decision": "approve", "body": "lgtm"})
            ivan = Http(stack.url, stack.credential("user:ivan", REPO, "integrator"))
            landed = ivan.ok("POST", f"/v1/repos/{REPO}/proposals/{pid}/integrate",
                             {"fencing_tokens": [lease.data["lease"]["fencing_token"]]})
            check(landed["outcome"]["status"] == "integrated", "a person integrated the proposal", landed)
            status = mcp.call("proposal_status", {"proposal_id": pid})
            check(status.data["verdicts"]["integrated"]["state"] == "ok", "the agent sees it integrated", status.text)
            now = mcp.call("read_file", {"path": "src/auth/login.rs"})
            check(now.data["content"] == NEW, "main has the agent's change")

            done = mcp.call("task_complete", {"task_id": task, "run_id": run})
            check(done.data["task"]["state"] == "done", "the task is done", done.text)
            fin = mcp.call("run_finish", {"run_id": run, "summary": "fixed"})
            check(fin.data["run"]["status"] == "completed", "the run is finished", fin.text)
            leases = mcp.call("lease_list", {})
            check(leases.data["leases"] == [], "finishing the run released the lease", leases.text)

            leaked = [line for line in mcp.stderr_lines if token in line]
            check(not leaked, "the credential never appears in ag-mcp's log")

    print(f"\n{PASSED} checks passed")


if __name__ == "__main__":
    main()
