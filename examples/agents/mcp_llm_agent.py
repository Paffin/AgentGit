#!/usr/bin/env python3
"""A real model drives `ag-mcp`: claim a task, edit a file, open a proposal. OPT-IN: it calls a paid API.

An agent loop of about eighty lines, with no agent framework: the tools of `ag-mcp` are offered to any
OpenAI-compatible chat-completions endpoint (OpenRouter by default) as functions, the model's tool calls are
forwarded to `ag-mcp` over stdio, the results go back to the model. A real `ag-server` is started on a temporary
directory, so the run is real end to end; afterwards the script checks from the outside what the model actually did.

    AG_LIVE_LLM=1 AG_LLM_MODEL=openai/gpt-4o-mini examples/agents/mcp_llm_agent.py

Settings (environment):
  AG_LIVE_LLM=1       required: without it the script says it is skipping and exits 0 (CI has no key)
  AG_LLM_MODEL        the model id the endpoint knows (required once AG_LIVE_LLM is set)
  AG_LLM_BASE_URL     default https://openrouter.ai/api/v1
  AG_LLM_API_KEY      sent as `Authorization: Bearer ...` when set; leave it unset behind a proxy that adds the
                      header itself
  AG_LLM_MAX_STEPS    model calls allowed, default 25

The model never gets the AgentGit credential: `ag-mcp` holds it. Exit status 0 only if a proposal that fixes the
file exists, authored by the agent's run, with `main` unmoved. Python 3 standard library only.
"""
import argparse
import json
import os
import sys
import urllib.error
import urllib.request

from agentgit_stack import Http, Stack, find_binary
from mcp_client import McpClient

REPO = "acme/web"
OLD = "pub fn login(user: &str) -> bool {\n    !user.is_empty()\n}\n"
TOOL_RESULT_LIMIT = 8000


def chat(base: str, key: str | None, body: dict) -> dict:
    req = urllib.request.Request(base.rstrip("/") + "/chat/completions", data=json.dumps(body).encode(), method="POST")
    req.add_header("Content-Type", "application/json")
    if key:
        req.add_header("Authorization", f"Bearer {key}")
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            return json.loads(r.read())
    except urllib.error.HTTPError as e:
        raise SystemExit(f"the model endpoint answered {e.code}: {e.read()[:500]!r}")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ag-server")
    ap.add_argument("--ag-mcp")
    ap.add_argument("--transcript", help="write the full transcript (JSON) to this file")
    args = ap.parse_args()
    if os.environ.get("AG_LIVE_LLM") != "1":
        print("skipped: set AG_LIVE_LLM=1 (and AG_LLM_MODEL) to run a real model against ag-mcp; it costs money")
        return
    model = os.environ.get("AG_LLM_MODEL")
    if not model:
        raise SystemExit("AG_LLM_MODEL is required (for example openai/gpt-4o-mini on OpenRouter)")
    base = os.environ.get("AG_LLM_BASE_URL", "https://openrouter.ai/api/v1")
    key = os.environ.get("AG_LLM_API_KEY")
    max_steps = int(os.environ.get("AG_LLM_MAX_STEPS", "25"))
    ag_mcp = find_binary("ag-mcp", args.ag_mcp)

    with Stack(args.ag_server) as stack:
        stack.create_repo(REPO, {"README.md": "# Web\n", "src/lib.rs": "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
                                 "src/auth/login.rs": OLD, "docs/guide.md": "Guide\n"})
        task = stack.create_task(REPO, "Reject blank user names",
                                 'login() accepts a user name made only of spaces; it should reject it.', ["src/auth/**"])
        token = stack.register_agent(REPO, "coder")
        main_before = stack.admin.ok("GET", f"/v1/repos/{REPO}/streams/head?stream=main")
        cmd = [ag_mcp, "--server", stack.url, "--repo", REPO, "--write-path", "src/**", "--write-path", "docs/**"]
        transcript: list[dict] = []
        served_model = model
        with McpClient(cmd, dict(os.environ, AG_MCP_TOKEN=token)) as mcp:
            instructions = mcp.discover()["instructions"]
            tools = [{"type": "function", "function": {"name": t["name"], "description": t["description"],
                                                       "parameters": t["inputSchema"]}} for t in mcp.list_tools()]
            messages = [
                {"role": "system", "content": "You are a software engineer working through AgentGit tools.\n\n" + instructions},
                {"role": "user", "content": "Find a ready task, claim it, make the change it asks for in the repository, and open a "
                                            "proposal for it. Look at the repository (tree, read_file) before you change anything and keep the change "
                                            "small. You cannot merge: stop once the proposal is open and say what you did."},
            ]
            for step in range(max_steps):
                reply = chat(base, key, {"model": model, "messages": messages, "tools": tools, "temperature": 0})
                served_model = reply.get("model", served_model)
                msg = reply["choices"][0]["message"]
                messages.append({k: v for k, v in msg.items() if k in ("role", "content", "tool_calls")})
                calls = msg.get("tool_calls") or []
                if not calls:
                    transcript.append({"step": step, "final": msg.get("content")})
                    break
                for c in calls:
                    name = c["function"]["name"]
                    try:
                        arguments = json.loads(c["function"]["arguments"] or "{}")
                    except ValueError:
                        arguments = {}
                    res = mcp.call(name, arguments)
                    transcript.append({"step": step, "tool": name, "arguments": arguments, "is_error": res.is_error,
                                       "result": res.text[:600]})
                    print(f"[{step}] {name}({json.dumps(arguments)[:160]}) -> {'ERROR ' if res.is_error else ''}{res.text[:100]!r}",
                          flush=True)
                    messages.append({"role": "tool", "tool_call_id": c["id"], "content": res.text[:TOOL_RESULT_LIMIT]})
            else:
                print(f"stopped after {max_steps} model calls")

            # What the model actually did, checked from outside the model.
            props = stack.admin.ok("GET", f"/v1/repos/{REPO}/proposals?state=open")
            props = props.get("proposals", props) if isinstance(props, dict) else props
            problems = []
            if len(props) != 1:
                problems.append(f"expected one open proposal, found {len(props)}")
            else:
                p = props[0]
                if not str(p.get("author", "")).startswith("agent:coder#"):
                    problems.append(f"the proposal's author is {p.get('author')!r}, not the agent's run")
                head = p.get("head") or p.get("head_changeset") or ""
                edited = mcp.call("read_file", {"path": "src/auth/login.rs", "rev": head})
                content = edited.data.get("content", "")
                if content == OLD or "trim" not in content:
                    problems.append(f"the proposal does not fix login.rs: {content!r}")
                print(f"\nproposal {p.get('id')} by {p.get('author')}; src/auth/login.rs in it:\n{content}")
            if stack.admin.ok("GET", f"/v1/repos/{REPO}/streams/head?stream=main") != main_before:
                problems.append("main moved without a human")

        calls_by_tool: dict[str, int] = {}
        for t in transcript:
            if "tool" in t:
                calls_by_tool[t["tool"]] = calls_by_tool.get(t["tool"], 0) + 1
        errors = sum(1 for t in transcript if t.get("is_error"))
        print(f"\nmodel {served_model}; {len(transcript)} transcript entries; tool calls {calls_by_tool}; tool errors {errors}")
        if args.transcript:
            with open(args.transcript, "w") as f:
                json.dump({"model": served_model, "transcript": transcript}, f, indent=2)
        if problems:
            print("FAILED: " + "; ".join(problems))
            sys.exit(1)
        print("OK: the model claimed the task, edited the file and opened a proposal; main did not move")


if __name__ == "__main__":
    main()
