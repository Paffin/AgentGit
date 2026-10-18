# Handoff: ag-mcp (MCP server track)

> **Superseded by [mcp_tail.md](mcp_tail.md)** (the work finished after this was written). Kept for history; where the two differ, the later file is right.

Status when this was written: the crate, ADR and three end-to-end suites are done and green locally; the guide, the
protocol/CLI conformance suites, the example client and the CI job are not.

## Shipped (crate `crates/ag-mcp`, binary `ag-mcp`, ADR `docs/adr/0008-mcp-server.md`)

- Protocol layer by hand (no SDK): newline-delimited JSON-RPC on stdio, bounded line reader, dual era (2026-07-28
  stateless + `initialize` for 2025-11-25/06-18/03-26), `server/discover`, `ping`, `tools/*`, `resources/*`
  (`agentgit://{owner}/{name}/{rev}/{path}`), `prompts/*`, cancellation, progress, concurrency limit.
- 31 tools in groups read / workspace / propose / coordinate / review / integrate (integrate only with
  `--enable-integrate`; no admin group). Allow-lists: `--allow`, `--repo`, `--write-path`, `--require-lease`.
- Results are data: control-sequence/bidi/invisible-char stripping, random-id fences, bounds (`--max-output-bytes`),
  credential scrubbing, explained refusals (`isError` + `_meta["agentgit/error"]`).
- Credential: `--token-file` (re-read after 401) or env var; never a CLI value.

## Tests (all pass locally: `cargo test -p ag-mcp`)

- unit tests in `src/` (framing, sanitizer, schema validator, verdicts, config, resources URIs, prompts)
- `tests/explore.rs`, `tests/agent_flow.rs`, `tests/security.rs`: real in-process `ag-server` + real `ag-mcp` over
  stdio (read-only exploration, task claim -> lease -> write -> propose -> verdicts, forbidden actions, credential
  expiry + rotation, truncation, hostile content). `tests/common/mod.rs` is the harness.

## Remaining (in this order)

1. `docs/guides/mcp.md` (install, Claude Code `claude mcp add`, Claude Desktop JSON, Agent SDK TS/Python snippets,
   security section, troubleshooting table; say "configuration verified by the stdio conformance tests; not
   exercised with a live model"). Add it to `docs/README.md`; restore the guide links in the ADR; run
   `AG=/home/user/ag-target/debug/ag python3 scripts/check_docs.py`.
2. `tests/protocol.rs`: binary-driven (discover, legacy `initialize` negotiation incl. unknown versions, -32022 /
   -32602 / -32700 / -32600 / -32601, batch rejected, `--max-message-bytes` oversized line, stdin EOF exit) and
   library-driven with custom tools (`Registry::from_tools` + `tools::def`): cancellation (no response after
   `notifications/cancelled`), progress notifications, `--max-concurrent` busy, panic in a tool, whole-line writes.
3. `tests/cli.rs`: `--list-tools`, `--check`, flag errors (`--allow admin`, missing token, `http://` non-loopback).
4. `examples/agents/mcp_scripted_agent.py` (stdlib only: starts `ag-server`, provisions with `ag-server admin ...`,
   drives `ag-mcp` over stdio) + a `mcp` job appended to `.github/workflows/ci.yml`.
5. Tell the release agent to add `ag-mcp` to the artefacts list; CHANGELOG/README/ROADMAP notes (see final report).

Known limits to document: no streamable HTTP (ADR-0008 explains why), no provenance signing, cancellation is best
effort, `--ca-file` replaces the built-in roots, `task_claim` twice by the same agent starts two runs.
