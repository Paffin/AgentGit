# Handoff: mcp_tail (the end of the MCP track)

Builds on `dev/handoff/mcp.md` (the crate, ADR-0008, three end-to-end suites). This package finished what that file listed
as remaining. Landed on `main` as `d03bd29`, `dbef52d`, `d13fe86`.

## State

- **Guide**: `docs/guides/mcp.md` (install, server-side preparation with commands that were run, Claude Code, Claude Desktop,
  Agent SDK TypeScript and Python, the tools by group, options, a security section with what the allow-list does and does
  not protect, a troubleshooting table, the recorded live run, limits). In `docs/README.md`, `mkdocs.yml`, FAQ and the two
  tutorials that pointed at the protocol overview. ADR-0008 links restored; its `--i-know-integrate-lands-code` was stale
  (the flag is `--enable-integrate`) and is corrected.
- **Tests** (`cargo test -p ag-mcp`: 54 unit + 6 agent_flow + 7 cli + 2 explore + 19 protocol + 2 run_credentials + 7 security):
  - `tests/protocol.rs`: discover; legacy `initialize` for all four revisions and unknown ones; `-32700/-32600/-32601/-32602/
    -32603/-32022`; batch rejected; `--max-message-bytes` (at the limit passes, one byte more is discarded and the stream
    resyncs); EOF exit; unterminated last line; with custom tools through `Registry::from_tools`: cancellation (no response),
    progress (token, forward only), `--max-concurrent` busy, a panicking tool, 200 concurrent calls never interleave a line.
  - `tests/cli.rs`: `--version`, `--list-tools` (needs no server or credential, same list as `tools/list`), `--check`, 13 flag
    errors, `--token-file`, and `--ca-file` against a real HTTPS `ag-server` (private CA accepted, built-in roots refuse it,
    a bundle with another certificate works, a wrong certificate fails, a file without certificates is named).
  - `agent_flow::claiming_a_task_twice_does_not_start_a_second_run`.
- **Examples** (`examples/agents/`): `mcp_scripted_agent.py` (27 checks, no model) runs in the new CI job `mcp`; `mcp_llm_agent.py`
  is opt-in (`AG_LIVE_LLM=1`, `AG_LLM_MODEL`), run in CI only on push when the repository defines the secret
  `OPENROUTER_API_KEY` and the variable `AG_LLM_MODEL`. Both start a real `ag-server` and drive the real `ag-mcp`.
- **Release**: `ag-mcp` is built, packed (`package_release.py` `BINARIES`), version-checked and listed in the release smoke,
  installed by `install.sh`, and in the Homebrew formula. The Scoop manifest is a template that no release feeds (no Windows
  archive yet) and was not changed.

## Evidence

- CI on `d13fe86`, all ten workflows success: CI https://github.com/Paffin/AgentGit/actions/runs/39123496165 (including the
  new job "MCP server with an agent"), Packaging .../39123495899, Docs .../39123495897, Windows .../39123495821,
  Containers .../39123495867, Security, Quality, CodeQL, Fuzz, OCI runtimes (all same sha).
- Live model run (2026-10-18, here, OpenRouter, `openai/gpt-4o-mini`, temperature 0): claimed the task, read the file, one
  schema-invalid `workspace_write_files` refused by argument validation, corrected, `propose` opened; the script verified one
  open proposal by `agent:coder#run_...` with `!user.trim().is_empty()` in `src/auth/login.rs` and `main` unmoved. An earlier
  attempt with a shorter prompt invented `login.js`; the script's outside check failed it. Both are in the guide.

## Known limits, triaged

- `task_claim` twice by one agent: **already refused by the server** (`start_run_with`: "task ... is already being worked on by
  run ..."); the old note in `mcp.md` was out of date. Now covered by a test; no code change.
- `--ca-file` replaces the built-in roots: **not fixed**. ureq 3 offers `RootCerts::{Specific, PlatformVerifier, WebPki}` and no
  union, and `webpki-roots` exposes trust anchors, not certificates, so an additive mode would mean replacing the HTTP/TLS
  client. Documented with the working alternative (a concatenated bundle, tested with a private CA plus another certificate;
  the system bundle itself was not tried).
- No audit item (`CONFIRMED.md`, `audit-final.json`) names `crates/ag-mcp` or the MCP docs; nothing to triage there.

## NOT verified

- The configuration snippets for Claude Code, Claude Desktop and both Agent SDKs were written from those hosts' documented
  formats and were not run in the hosts.
- The live model step has run once, by hand. CI does not run it (no key); the first CI run with the secret may need the
  `AG_LLM_BASE_URL`/header set-up for the provider in use (the script sends `AG_LLM_API_KEY` as a bearer token).
- Release workflow edits (ag-mcp in the smoke and the `install.sh` step) were not run: that workflow runs on tags and manual
  dispatch only. The same pack/install code paths are covered by `scripts/test_release_scripts.py` (36 tests, pass) and the
  Packaging workflow. A rehearsal run from the Actions UI would confirm it.
- `mkdocs build --strict` was not run locally (mkdocs is not installed here); the Docs workflow was green on `d13fe86`.
- Streamable HTTP, provenance signing: out of scope, see ADR-0008.

## Next

1. Run the Release rehearsal once from the Actions UI to see ag-mcp in the archives on the four targets.
2. Define `OPENROUTER_API_KEY` and `AG_LLM_MODEL` to make the live job real, or leave it opt-in.
3. Try the snippets in Claude Code and Claude Desktop and fix the guide where reality differs.
4. If an additive CA option matters, it needs a different TLS client setup; open an ADR before changing the dependency.
