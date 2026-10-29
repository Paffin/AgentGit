# Giving an agent AgentGit through MCP

`ag-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server for the **remote** AgentGit workflow.
An MCP client (Claude Code, Claude Desktop, the Claude Agent SDK, any other host) starts it as a child process and
gets tools to explore a repository, claim a task, take a lease, edit files in a server-side workspace and open a
proposal. It cannot land code unless you switch that on, and it cannot touch policy, roles or credentials at all.

It is a thin client of a running `ag-server` (AGWP over HTTP/JSON): no checkout, no local `.ag/` directory, nothing
to keep in sync. The decisions behind it are in [ADR-0008](../adr/0008-mcp-server.md); run-bound credentials, which it
mints for every run it starts, are in [ADR-0010](../adr/0010-run-bound-credentials.md).

**What is verified, and how.** The tools, the protocol layer and the allow-list are tested against a real in-process
`ag-server` and the real `ag-mcp` binary over stdio (`crates/ag-mcp/tests/`: `explore`, `agent_flow`, `security`,
`run_credentials`, `protocol`, `cli`). A scripted client
([`examples/agents/mcp_scripted_agent.py`](../../examples/agents/mcp_scripted_agent.py)) runs the whole task flow in CI on
every push. One real model has driven it (see [Run it with a real model](#run-it-with-a-real-model)). **Not
verified:** the configuration snippets for Claude Code, Claude Desktop and the Agent SDKs below follow those hosts'
documented formats but were not run in those hosts here.

## 1. Install

`ag-mcp` ships in the release archive next to `ag` and `ag-server` (`scripts/install.sh` installs all of them), or build
it:

```bash
cargo build --release -p agentgit-mcp        # target/release/ag-mcp
ag-mcp --version
```

It needs nothing else: no Node, no Python, no network except the route to your `ag-server`.

## 2. Prepare the server side

The credential is the security boundary, so make a narrow one. With the offline admin tool (on the server host):

```bash
ag-server admin create-repo acme/web
ag-server admin grant --subject agent:coder --repo acme/web --role writer
ag-server admin mint  --subject agent:coder --repo acme/web --ttl 3600 | tail -n 1 > agent.token
chmod 600 agent.token
```

`writer` lets the agent propose; it does not let it integrate or review. `mint` prints the token once; `--ttl` is
the lifetime in seconds. A credential can be narrowed further with `--stream` and `--operation` (repeatable). Register
the agent once, with the **public** half of an Ed25519 key (the MCP server never signs anything, see
[Limits](#limits)):

```bash
openssl genpkey -algorithm ed25519 -out coder.pem
PUB=$(openssl pkey -in coder.pem -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n')
curl -sS -X POST "$AG_URL/v1/repos/acme/web/agents" -H "Authorization: Bearer $(cat agent.token)" \
     -H 'Content-Type: application/json' -d "{\"name\":\"coder\",\"public_key\":\"$PUB\"}"
```

Then check what the process will be allowed to do, before any host is involved:

```text
$ ag-mcp --server https://git.example.com --token-file agent.token --repo acme/web --check
server    https://git.example.com (AGWP agwp.v1)
identity  agent:coder
roles     ["acme/web:writer"]
scope     {"operations":[],"repos":["acme/web"],"run_id":"","streams":[]}
expires   in 59 min
groups    read,workspace,propose,coordinate,discuss
tools     32
```

`--check` exits non-zero with the reason if the server is unreachable, the credential is rejected or expired.
`ag-mcp --server URL --list-tools` prints the tools that would be offered, as JSON, and needs neither a server nor a
credential.

## 3. Configure a host

All hosts run the same thing: the command `ag-mcp` with arguments and the credential in the environment. The
credential is never an argument (it would show in process listings): use `AG_MCP_TOKEN_FILE` (re-read after a `401`,
so you can rotate it without restarting the session) or `AG_MCP_TOKEN` (read once).

### Claude Code

```bash
claude mcp add --transport stdio --env AG_MCP_TOKEN_FILE=$HOME/.config/agentgit/agent.token \
  agentgit -- ag-mcp --server https://git.example.com --repo acme/web --write-path 'src/**' --write-path 'docs/**'
```

or commit a `.mcp.json` to the project (leave the token file out of the repository):

```json
{
  "mcpServers": {
    "agentgit": {
      "command": "ag-mcp",
      "args": ["--server", "https://git.example.com", "--repo", "acme/web", "--write-path", "src/**"],
      "env": { "AG_MCP_TOKEN_FILE": "/home/me/.config/agentgit/agent.token" }
    }
  }
}
```

### Claude Desktop

`claude_desktop_config.json` (Settings, Developer, Edit config); restart the app afterwards:

```json
{
  "mcpServers": {
    "agentgit": {
      "command": "/usr/local/bin/ag-mcp",
      "args": ["--server", "https://git.example.com", "--repo", "acme/web"],
      "env": { "AG_MCP_TOKEN_FILE": "/Users/me/.config/agentgit/agent.token" }
    }
  }
}
```

Desktop hosts start the process with a minimal `PATH`: give an absolute path to the binary.

### Claude Agent SDK, TypeScript

```ts
import { query } from "@anthropic-ai/claude-agent-sdk";

for await (const message of query({
  prompt: "Claim the next ready task, make the change and open a proposal.",
  options: {
    mcpServers: {
      agentgit: {
        type: "stdio",
        command: "ag-mcp",
        args: ["--server", "https://git.example.com", "--repo", "acme/web", "--write-path", "src/**"],
        env: { AG_MCP_TOKEN_FILE: "/home/me/.config/agentgit/agent.token" },
      },
    },
    allowedTools: ["mcp__agentgit__*"],
  },
})) {
  if (message.type === "result") console.log(message.result);
}
```

### Claude Agent SDK, Python

```python
import asyncio
from claude_agent_sdk import ClaudeAgentOptions, query

options = ClaudeAgentOptions(
    mcp_servers={
        "agentgit": {
            "type": "stdio",
            "command": "ag-mcp",
            "args": ["--server", "https://git.example.com", "--repo", "acme/web", "--write-path", "src/**"],
            "env": {"AG_MCP_TOKEN_FILE": "/home/me/.config/agentgit/agent.token"},
        }
    },
    allowed_tools=["mcp__agentgit__*"],
)

async def main():
    async for message in query(prompt="Claim the next ready task, make the change and open a proposal.", options=options):
        print(message)

asyncio.run(main())
```

Any other MCP host: a stdio server with that command, arguments and environment. `ag-mcp` speaks protocol
2026-07-28 (stateless, `server/discover`) and, for hosts that have not moved yet, the `initialize` handshake of
2025-11-25, 2025-06-18 and 2025-03-26; an unknown requested version is answered with the newest handshake revision
and the client decides.

## 4. What the agent gets

Tools are organized by permission group. The default is `read,workspace,propose,coordinate,discuss`; `--allow` names the groups explicitly.

| Group | Tools |
|---|---|
| `read` | `whoami`, `repo_list`, `repo_info`, `log`, `show`, `blame`, `tree`, `read_file`, `diff`, `predict_conflicts`, `proposal_list`, `proposal_status`, `task_list`, `lease_list`, `queue_status`, `workspace_list`, `discussion_list`, `discussion_replies` (all read-only) |
| `workspace` | `workspace_create`, `workspace_write_files` (one atomic snapshot per call), `workspace_sync`, `workspace_delete` |
| `propose` | `propose`, `proposal_update`, `proposal_close`, `request_validation` |
| `coordinate` | `task_claim`, `run_start`, `task_complete`, `run_heartbeat`, `run_finish`, `lease_acquire`, `lease_renew`, `lease_release` |
| `discuss` | `discussion_start`, `discussion_reply`, `discussion_resolve`: exact-revision threads with version-bound resolution; no approval or integration rights |
| `issue` (off unless named) | `issue_import_preview`, `issue_import_apply`: inspect a normalized import manifest, then explicitly apply its digest; no provider credentials |
| `review` (off unless named) | `review`: approve, request changes or comment on a proposal's head. Keep it off for an agent that also authors changes: an author must not review its own work |
| `integrate` (only with `--enable-integrate`) | `integrate`: lands a proposal if policy allows. It logs a warning at start and on every call |

There is no `admin` group: policy, roles and credentials are deliberately unreachable (`--allow admin` is refused with
that explanation). Resources (`agentgit://{owner}/{name}/{rev}/{path}`, files and directories) and four prompts
(`work_a_task`, `review_proposal`, `resolve_conflicts`, `explore_repo`) are offered only when their required
groups are enabled. Exploration needs `read`; task work also needs `workspace`, `propose` and `coordinate`;
review needs `review`; conflict resolution needs `workspace`.

The task and review prompts read complete line-discussion pages before resolving a thread. With `discuss`
enabled, authors can address feedback and report their changes, while reviewers discuss the inspected revision
without needing permission to edit it. Without `discuss`, these prompts only read existing discussions.
Thread resolution does not approve or integrate a proposal. Task completion follows the requested outcome:
if the task requires landed code, publishing a proposal alone does not complete it.

A run is explicit, because MCP 2026-07-28 has no sessions: `task_claim` (or `run_start`) returns a `run_id`, and the
agent passes it to the calls that act as that run. Two claims of one task are not two runs: the second is refused with
the run that holds it. By default each run also gets a credential of its own, bound to that run and kept in the
process's memory (`--run-credentials auto|required|off`), so another run of the same agent cannot act as it.

Results are data, not instructions: a one-line summary written by `ag-mcp`, then repository text inside fences that
carry a random id (`<<<AGENTGIT-DATA 3fa9… kind=file.content>>>`), plus the same data as `structuredContent`. Terminal
control sequences, bidi overrides and invisible characters are stripped (and counted in the result), output is bounded
by `--max-output-bytes` (default 64 KiB) with `truncated: true`, and anything shaped like a credential is scrubbed.
A refused call comes back as an explained tool error (`isError`, with a machine-readable code in
`_meta["agentgit/error"]`), not as a bare "unknown tool".

## 5. Narrowing it: the options

| Option (environment) | Effect |
|---|---|
| `--repo owner/name` (`AG_MCP_REPO`, repeatable, `owner/*` allowed) | Only these repositories. With exactly one, tools default to it |
| `--allow GROUPS` (`AG_MCP_ALLOW`) | Which groups are offered |
| `--write-path GLOB` (`AG_MCP_WRITE_PATHS`) | Writes must match one of the globs (`*` stays inside a directory, `**` crosses) |
| `--require-lease` (`AG_MCP_REQUIRE_LEASE`) | Writes only under an active lease the agent holds |
| `--enable-integrate` | Offers `integrate` |
| `--run-credentials` (`AG_MCP_RUN_CREDENTIALS`) | `auto` (default), `required` (refuse to start a run without a bound credential) or `off` |
| `--max-lease-seconds`, `--max-output-bytes`, `--max-concurrent`, `--max-message-bytes`, `--timeout-secs` | Bounds |
| `--ca-file PEM` (`AG_MCP_CA_FILE`) | Trust this CA **instead of** the built-in roots |
| `--insecure-http` | Allow `http://` to a host that is not loopback. The credential then travels in clear text |
| `--log-level` (`AG_MCP_LOG`) | `error`, `warn`, `info` (default) or `debug`; logs go to standard error only |

`http://` is refused except for loopback; redirects are never followed (the bearer token would travel with them).

## 6. Security: what the allow-list protects, and what it does not

**The boundary is the credential on the server, not `ag-mcp`.** The credential's roles, repository scope, stream
scope and operation list are enforced by `ag-server`, and so is policy. Everything `ag-mcp` adds can only narrow
that, and it narrows it **inside one process that the agent does not control**.

What the options do protect:

- The tool groups limit what the model can ask for. A tool outside the allow-list is not listed, and calling it
  anyway is refused with an explanation before anything is sent.
- `--repo`, `--write-path` and `--require-lease` are checked before a request leaves the process, with the reason in
  the refusal. A write to `Cargo.toml` when only `src/**` is allowed never reaches the server.
- The credential is never printed, logged or returned, and every outgoing string is scrubbed of it.
- Repository content cannot pose as markup of the protocol (fences with a per-response id, control characters
  stripped), and a hostile file cannot grow a response without bound.
- An agent that opens a proposal still needs the policy to be satisfied (checks, approvals, leases) and a person or
  integrator to land it. The scripted example shows exactly that.

What they do **not** protect:

- **They are not a barrier against anyone who can read the credential.** If the agent also has a shell, a file-read
  tool or any other way to read `agent.token`, it can call the server directly and `--allow`, `--write-path` and
  `--require-lease` mean nothing. Keep the token file readable by `ag-mcp`'s user only, and keep that user away from
  the model's other tools, or put the limits you need in the credential itself: `ag-server admin mint --stream ... --operation ...`.
  `--write-path` and `--require-lease` in particular are checks inside `ag-mcp`; the server does not know about them.
- **They do not stop prompt injection.** Text in a file, a commit message or a task description can still persuade a
  model. Fencing makes it inert as markup and clearly labelled as data; it cannot make a model ignore it. This is why
  the default has no `integrate`, no `review`, and why a policy that needs a human approval is the real answer to a
  hijacked agent: it can *propose*, nothing more.
- **Host-side auto-approval.** MCP tool annotations (`readOnlyHint`, `destructiveHint`) are hints for the host.
  `workspace_delete` and `proposal_close` are marked destructive; do not auto-approve them if you do not want them
  run unattended.
- **Cancellation is best effort.** A call already sent to the server is not undone; the answer is simply not
  delivered. The agent should re-read state after a cancelled call.
- **The transport is local.** There is no HTTP transport: whoever can run the process (the host) is trusted with its
  credential. A remote deployment runs `ag-mcp` next to the agent, pointing at a remote `ag-server` over HTTPS.

## 7. Troubleshooting

Run `ag-mcp ... --check` first: it is the same code path with a readable report. Hosts show the process's standard
error (the log) in their MCP logs; add `--log-level debug` to see every request. Standard output carries protocol
messages only.

| Symptom | Cause and fix |
|---|---|
| The host says the server failed to start | Run the exact command in a shell. `the environment variable AG_MCP_TOKEN is not set`: pass `AG_MCP_TOKEN_FILE` or `AG_MCP_TOKEN` in the host's `env` block, not in your shell. Desktop hosts may not read your shell profile or `PATH` |
| `refusing to send the credential over plain http` | Use `https://`, or `--insecure-http` if the network path is trusted. Loopback is always allowed |
| `cannot reach the server` | Wrong URL, a firewall, or a private CA: for a certificate error add `--ca-file ca.pem` |
| `the server rejected the credential` / tool error `credential_rejected` | Expired, revoked or wrong credential. With `--token-file` write the new token to the file; the next call re-reads it after a `401` |
| `run_ended` | The run the call acted as is finished, failed or was reaped as abandoned, and its bound credential stopped working with it. Start a new run with `task_claim` or `run_start`; saved workspaces and proposals are still there |
| `permission_denied` | The server refused: the credential's scope or the identity's role does not allow it. `ag-mcp` cannot work around that, by design |
| `repo_not_allowed` | The repository is outside `--repo`. Only the repositories you listed can be used |
| `tool_disabled` | The group is not in `--allow` (`integrate` needs `--enable-integrate`, `review` must be named) |
| `path_not_allowed` / `path_outside_lease` | The write is outside `--write-path`, or not covered by a lease the agent holds (`--require-lease`): acquire a lease for it first |
| `lease_conflict` | Another agent holds an exclusive lease on an overlapping scope: coordinate, or choose another scope. A soft lease always succeeds |
| `lease_too_long` | The request exceeds `--max-lease-seconds`; ask for less and renew |
| `busy` | More than `--max-concurrent` calls at once. Retry |
| `run_credential_required` | `--run-credentials required` and the server (or the credential) cannot bind a run. Use `auto`, or a server that supports run-bound credentials and an agent credential |
| `this credential authenticates as ..., which is not an agent` | `task_claim` and `run_start` need an agent credential, or the `agent` argument naming the agent to run as |
| `-32600 message larger than the limit` | A request line exceeded `--max-message-bytes`; the line was discarded and the session continues |
| A result says `truncated: true` | The output limit was hit. Ask for a narrower range (`read_file` with `start_line`/`end_line`, `tree` with a path) or raise `--max-output-bytes` |
| `-32022` unsupported protocol version | The host asked for a revision `ag-mcp` does not speak; the error lists the supported ones |

## Run it with a real model

[`examples/agents/`](../../examples/agents/README.md) has two clients that start a real `ag-server` on a temporary
directory and drive `ag-mcp` over stdio:

- `mcp_scripted_agent.py` is a fixed script with no model. It claims a task, takes an exclusive lease, reads and edits a
  file in a workspace, opens a proposal, tries the writes the allow-list must refuse, tries the `integrate` tool that
  is not offered, then lets a person review and land the proposal and checks that the agent sees it. CI runs it on every push.
- `mcp_llm_agent.py` puts a model in the loop: about eighty lines of agent loop over any OpenAI-compatible
  chat-completions endpoint (OpenRouter by default). It is opt-in: without `AG_LIVE_LLM=1` it prints that it is
  skipping. CI runs it only when the repository defines the `OPENROUTER_API_KEY` secret and the `AG_LLM_MODEL`
  variable; there is none today, so CI does not exercise it.

```bash
AG_LIVE_LLM=1 AG_LLM_MODEL=openai/gpt-4o-mini python3 examples/agents/mcp_llm_agent.py
```

**Evidence of one live run** (2026-10-18, model `openai/gpt-4o-mini` on OpenRouter, temperature 0, `ag-mcp` built from this
tree, a fresh `ag-server`; the task: "login() accepts a user name made only of spaces; it should reject it"):

```text
[0] task_list(ready_only)        -> 1 task
[1] task_claim                   -> run agent:coder#run_... started
[2] repo_info, [3-5] tree x3     -> the model walks / , src, src/auth
[6] read_file src/auth/login.rs  -> the three-line function
[7] workspace_create             -> workspace on main
[8] workspace_write_files        -> ERROR invalid_arguments: message: required (refused before the server saw it)
[9] workspace_write_files        -> snapshot written (the model fixed its call)
[10] propose                     -> proposal opened, verdicts returned
[11] final answer                -> says what it did and stops
```

The script then checked from outside the model: exactly one open proposal, authored by `agent:coder#run_...`, whose
`src/auth/login.rs` reads `!user.trim().is_empty()`, and `main` unmoved. 11 tool calls, one of them a schema error
that the argument validation caught and the model recovered from.

Two honest notes. The first attempt of the same model, with a shorter prompt, never looked at the repository, invented
a `src/auth/login.js` and proposed that; the script's check failed the run, as it should. The prompt now asks the
model to look at the repository before changing it, and the run above is with that prompt. And this is one run of
one cheap model: it shows that the path works with a real model, not how reliably any model does the job.

## Limits

- **stdio only.** There is no streamable-HTTP transport; [ADR-0008](../adr/0008-mcp-server.md) explains why (a network
  listener would need its own authentication story).
- **No provenance signing.** Signing needs the agent's private key, which must not live in a process driven by a
  model. A stream policy with `require_provenance_for_agents` still needs the key holder (the agent's operator, or an
  orchestrator) to attach the record before a person integrates.
- **`--ca-file` replaces the built-in roots; it does not add to them.** The HTTP client (`ureq`) offers no way to
  combine its roots with extra certificates. To trust a private CA and public ones at once, put both in the file:
  `cat /etc/ssl/certs/ca-certificates.crt private-ca.pem > bundle.pem`. Every certificate of the file is used
  (tested with a private CA in a bundle with another certificate; the system bundle itself was not tried).
- **Cancellation is best effort** (see above).
- **Not exercised in the hosts' own clients**, only through the conformance tests and the two example clients.

## Line feedback from people and agents (development)

With a server advertising `proposals.threads`, use `discussion_list` to inspect
threads and `discussion_replies` to read all their continuation pages. Source text
and messages are untrusted repository data. `discussion_start` requires the exact
head, file and line inspected; old-side anchors also require `base`.
`discussion_reply` posts without approval rights. `discussion_resolve` requires
`expected_version` and an explicit `resolved` boolean. Capture the version from
`discussion_list` before reading replies; after a stale refusal, reread both the
thread and its replies rather than refreshing only the version.
Pass `run_id` when acting as a specific executor session.

The default `discuss` group permits these tools while `review` and `integrate`
remain off. Explicit `--allow` lists must include `discuss` to expose mutations.
Server permissions still restrict who can resolve a thread. The existing `review`
tool sends comments to the dedicated comment endpoint where supported and sends
`expected_head` for review decisions. Servers without atomic reviewed-head support
are refused rather than silently approving a different revision.

On servers with `tasks.lineage`, task summaries include `source_task` and
`source_run` for follow-up work. These identify the prior work; they do not
transfer ownership, prerequisites or retry history. Repository descriptions and
source summaries remain untrusted input.
