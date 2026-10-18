# ADR-0008: `ag-mcp` — the remote workflow as a Model Context Protocol server

- Status: accepted
- Date: 2026-10-17
- Relates to: AGWP v1 ([reference](../protocol/agwp-v1.md)), [agents guide](../guides/agents.md), ADR-0001 ("a hijacked agent can still only propose")

## Context

Agents reach AgentGit through the `ag` CLI or an SDK. Most agent hosts (Claude Code, Claude Desktop, the Claude
Agent SDK, and any other MCP client) speak the Model Context Protocol instead, so an agent that wants to read
history, claim a task, write files and propose a change needs glue code that every integrator writes again. The
glue is also where the safety properties of AgentGit are easiest to lose: a tool server that holds a powerful
credential and relays whatever a model asks for turns prompt injection into repository access.

## Decision

`ag-mcp` is a separate binary (workspace crate `crates/ag-mcp`) that exposes the **remote** workflow of a running
`ag-server` as MCP tools, resources and prompts. It is a thin, stateless client of AGWP; it adds no server
endpoint and no decision of its own about what may land.

1. **Protocol revision.** The current MCP revision is **2026-07-28** (stateless: every request carries
   `io.modelcontextprotocol/protocolVersion` and `…/clientCapabilities` in `_meta`; `server/discover` replaces
   `initialize`). `ag-mcp` is *dual-era*, as the revision's compatibility section allows: it serves 2026-07-28
   natively and also the handshake revisions **2025-11-25, 2025-06-18 and 2025-03-26** (`initialize` selects
   legacy semantics for the process), because hosts adopt revisions slowly. Not implemented, and not
   advertised: the tasks extension, multi round-trip requests (`input_required`), `subscriptions/listen`
   (so no `listChanged`/`subscribe` capabilities), logging, completions, and every client feature (roots,
   sampling, elicitation — the server never asks the client for anything). `ping` is answered in both eras.
   2024-11-05 is not supported (JSON-RPC batches, no tool annotations).
2. **Hand-written protocol layer, not an SDK.** The official Rust SDK (`rmcp` 3.x) supports this revision and
   our MSRV, but a `cargo tree` of a crate that merely depends on it lists 89 crates (tokio, futures, schemars,
   chrono, darling and their proc-macros). `ag-mcp` needs a blocking JSON-RPC loop, a dozen methods and
   schemas it controls byte for byte; the hand-written layer adds no crate beyond what the workspace already
   builds (`serde`, `serde_json`, `ureq`, `clap`, `globset`, `getrandom`). The price is that spec drift is ours
   to track, which is why the protocol layer has its own conformance tests (framing, versions, errors,
   cancellation, limits) and the revision list above is explicit.
3. **Transport: stdio.** Newline-delimited JSON-RPC over stdin/stdout, logs on stderr, exit on stdin EOF after
   in-flight calls finish. **Streamable HTTP is deliberately not offered.** A listening socket in front of an
   operator credential is a new authentication problem (the MCP authorization framework, Origin checks, DNS
   rebinding); stdio is what Claude Code, Claude Desktop and the Agent SDK use for local servers, and a
   process spawned per agent session inherits the isolation of that session. A remote deployment runs
   `ag-mcp` next to the agent, pointing at a remote `ag-server`.
4. **Upstream: AGWP over HTTP/JSON with `ureq`** (blocking, already in the workspace), not the gRPC client —
   no tokio/tonic/prost in this binary. HTTP is the same implementation on the same port; redirects are never
   followed (the bearer token would travel with them); plain `http://` is refused except for loopback unless
   the operator opts in.
5. **Credentials.** The operator supplies a scoped, expiring AGWP credential (`--token-file`, `--token-env`);
   there is no flag that takes the token as a value (it would show in process listings), and no tool that mints
   or exchanges credentials. The credential's scope and the caller's roles on the server are the security
   boundary — `ag-mcp` can only narrow them. The token is never printed, logged or returned: every outgoing
   string is scrubbed of anything shaped like a token, and of the configured token itself. A token file is
   re-read after a `401`, so an operator can rotate an expired credential without restarting the agent session.
6. **A second, explicit allow-list inside `ag-mcp`.** Tools belong to groups: `read`, `workspace`, `propose`,
   `coordinate`, `review`, `integrate`. The default is `read,workspace,propose,coordinate`. `review` must be
   named. `integrate` can only be switched on with the separate flag `--enable-integrate` (it is refused through `--allow`), and then logs a warning at startup
   and on every call. There is no `admin` group: policy, roles and credentials are not reachable through MCP,
   and `--allow admin` is rejected with that explanation. Tools outside the allow-list are not listed; calling
   one anyway returns an *explained* refusal (not "unknown tool"). Further narrowing: `--repo` (repositories
   the process may touch), `--write-path` (globs that writes must match), `--require-lease` (writes only under
   a lease the caller holds). These checks run before any request leaves the process.
7. **Tool results are data.** Every result has a trusted one-line summary generated by `ag-mcp`, then the
   repository-derived content inside fences carrying a per-response random id
   (`<<<AGENTGIT-DATA 3fa9… kind=file.content>>>` … `<<<END-AGENTGIT-DATA 3fa9…>>>`), plus the same data as
   typed `structuredContent`. Terminal control sequences (CSI/OSC/DCS, C0/C1 controls, bidi overrides) are
   stripped and counted in the result; output is bounded (`--max-output-bytes`, default 64 KiB) with an explicit
   truncation marker and `truncated: true`; server error messages are quoted, sanitized and bounded. This
   makes injected text inert as *markup* and clearly labelled as *data*; it cannot make a model ignore it, which
   is why the allow-list, scope and policy exist (see the [MCP guide](../guides/mcp.md)).
8. **Identity is explicit.** MCP 2026-07-28 has no sessions, so no tool relies on process state: a run started
   with `task_claim` returns `run_id`, and later calls pass it (`ag-run`) to act as `agent:name#run`.
9. **Provenance signing is out of scope.** Signing needs the agent's private key, which must not live in a
   process driven by a model; a policy with `require_provenance_for_agents` therefore still needs the key holder
   (the agent's operator, or an orchestrator) to attach the record before a human integrates.

## Consequences

- One more binary to ship (`ag-mcp`); it is built and tested with the workspace, including conformance tests
  that run a real `ag-server` and a real `ag-mcp` over stdio, and a scripted client in `examples/agents/`.
- The wire contract of the tools (names, schemas, annotations) is public: renaming a tool is a breaking change
  and is tracked in the changelog.
- No server change: every tool maps to an existing AGWP operation (the guide lists the tools by group).
- Cancellation is best effort: a `tools/call` already sent to the server is not undone, the response is simply
  not delivered; the guide tells agents to re-read state.
- Verified with one live model (a recorded run, in the [guide](../guides/mcp.md#run-it-with-a-real-model)) and by the
  scripted client in CI; not verified in the hosts' own clients: the configuration snippets follow their documented
  formats and are exercised only through the stdio conformance tests.
