# AgentGit documentation

| If you want to… | Read |
|---|---|
| install it (release archive, container image, from source) | [Install](getting-started.md#install) |
| try it in five minutes | [Getting started](getting-started.md) |
| follow a walkthrough (every command in them is run by the tests) | [Tutorials](tutorials/README.md): [your first agent-safe repository](tutorials/first-agent-safe-repo.md), [run a team server](tutorials/team-server.md), [policy cookbook](tutorials/policy-cookbook.md), [migrating a Git repository](tutorials/migrating-from-git.md), [troubleshooting a blocked integration](tutorials/troubleshooting-blocked-integration.md) |
| understand the vocabulary (changeset, stream, proposal, intent…) | [Concepts](concepts.md), [Glossary](glossary.md) |
| get a quick answer | [FAQ](faq.md) |
| see how it differs from Git, GitHub, GitLab, Bitbucket, Gerrit and Jujutsu | [AgentGit compared](comparison.md) |
| run agents safely against a shared repository | [Agents guide](guides/agents.md) |
| give an agent AgentGit through MCP (Claude Code, Claude Desktop, the Agent SDKs) | [MCP guide](guides/mcp.md): install, configuration, what the allow-list protects and what it does not, troubleshooting, a recorded run with a real model |
| run the Go agent platform (scheduler, sessions, advisor, reviewers, semantic index; optional model mode) | [Orchestrator guide](guides/orchestrator.md) |
| call the server from TypeScript or Python | [Client SDKs](guides/sdks.md) and the [AGWP v1 reference](protocol/agwp-v1.md) |
| decide what must be true before code lands | [Policy reference](guides/policy.md) and [Validation](guides/validation.md) |
| work against a server (clone, pull, push) and offline | [Remote guide](guides/remote.md) |
| read history: blame, renames, path history | [History guide](guides/history.md) |
| watch agents work, review and integrate in a browser | [Web UI guide](guides/web-ui.md); why it is laid out as it is: [Web UI review](design/web-ux-review.md) |
| run checks on untrusted code and sign the result | [Workers guide](guides/workers.md); what the [sandbox](architecture/sandbox.md) does and does not stop |
| move to or from Git | [Git bridge](guides/git-bridge.md) |
| look up a command, a help topic or a `--json` shape | [CLI reference](reference/cli.md), [help topics](reference/help-topics.md), [`--json` shapes](reference/cli-json.md) *(all generated)* |
| find out where `ag` reads its configuration, or install shell completions and man pages | [Configuration](reference/configuration.md), [Completions and man pages](reference/completions.md) |
| understand how it works and why it is safe | [Architecture overview](architecture/overview.md), [Merge semantics](architecture/merge-semantics.md), [Storage](architecture/storage.md) |
| see how each promise of the architecture decision maps to code and tests, and what is not built | [ADR traceability](architecture/adr-traceability.md) |
| check a guarantee against the code and the test that proves it | [Publication & integration](architecture/publication-and-integration.md), [Security](architecture/security.md) (with what fuzzing found), [Testing](architecture/testing.md) (what runs where, coverage, mutation scores, test doubles) |
| implement a compatible reader/writer | [Object formats](architecture/object-format.md), [Compatibility](architecture/compatibility.md) (what a newer build promises about old data, the fixtures that prove it) |
| see what the engine and the server cost, with the numbers and how they were taken | [Performance of the engine](architecture/performance.md) (1k/10k/100k files), [Performance under load](operations/performance.md) (HTTP/JSON, gRPC, SSE, rate limits) |
| operate a repository | [Durability & backup](operations/durability.md), [GC & retention](operations/gc-and-retention.md), [PostgreSQL & S3](operations/postgres-and-s3.md), [High availability](operations/high-availability.md) (several replicas: guarantees, failure modes, upgrade), [Troubleshooting](operations/troubleshooting.md), [Windows](operations/windows.md) (support matrix, path and checkout rules) |
| deploy and monitor the server | [Deployment](operations/deployment.md) (binary, Compose, Kubernetes), [Observability](operations/observability.md), [Releasing & distribution](operations/releasing.md) (release pipeline and its rehearsal, installers, SBOMs, what is not proven), [Hardening checklist](operations/hardening.md) (every setting and default checked against the server), [Supply chain](operations/supply-chain.md) (vulnerability, licence and workflow gates, accepted exceptions) |
| talk to the server from your own code | [AGWP v1 reference](protocol/agwp-v1.md), [`openapi.json`](protocol/openapi.json) |
| see what is built and what is planned | [Roadmap](../ROADMAP.md), [Protocol overview](protocol/README.md) |
| learn why decisions were made (identity names, run-bound credentials, paging indexes and background leases, job-claim fencing: ADR-0009 to 0012) | [ADRs](adr/README.md) |

## Keeping the docs true

Documentation that drifts is worse than none, so most of it is *checked*:

- [`reference/cli.md`](reference/cli.md), [`help-topics.md`](reference/help-topics.md),
  [`cli-json.md`](reference/cli-json.md), the shell completions and the man pages under `reference/` are **generated**
  from the binary and its golden test fixtures by `scripts/gen_cli_docs.py`; CI fails if any of them is stale.
- The commands in the [tutorials](tutorials/README.md#how-these-tutorials-are-tested) are **executed** against the real
  binaries (`scripts/doc_snippets.py`), the output the pages show is compared with what the commands print, and the
  policy documents in the cookbook go through the real policy parser.
- The walkthrough in the README comes from [`examples/quickstart.sh`](../examples/quickstart.sh), which CI
  runs as a test (and so does `cargo test`). The script asserts every exit code, and `crates/ag-cli/tests/quickstart.rs`
  checks that every line of output shown in the README's "See it work" transcript is a line the script really prints
  (ids and text cut short with `…` are wildcards), so a reworded message fails the test.
- `scripts/check_docs.py` checks links, test names, `ag` commands, environment variables and the refusal messages quoted
  in the docs against the code; the site build (`mkdocs build --strict`) checks every link and `#anchor`.
- Byte-level format claims are backed by [`tests/conformance/`](../tests/conformance/): vectors produced by an
  independent Python implementation and verified against the Rust code.
- Every guarantee in the architecture docs names the test that exercises it.
- `scripts/check_no_stubs.py` keeps stubs out of the product code: no `todo!()`, `unimplemented!()`, `TODO`/`FIXME`
  markers or "for now" placeholders in production sources, except the entries (each with a reason) in
  `scripts/no_stubs_allowlist.txt`.

If you change behaviour, change the doc in the same pull request.

## Building this site

The pages are plain Markdown that reads well on GitHub; [MkDocs](https://www.mkdocs.org) with the Material theme turns them
into a static site with offline search (`mkdocs.yml`; `docs/requirements.txt` pins every dependency and its hash):

```sh
python3 -m venv .venv-docs && .venv-docs/bin/pip install --require-hashes -r docs/requirements.txt
.venv-docs/bin/mkdocs serve             # live preview at http://127.0.0.1:8000
.venv-docs/bin/mkdocs build --strict    # what CI runs: a broken link, anchor or nav entry fails the build
```

`.github/workflows/docs.yml` builds it on every push and pull request. It deploys to GitHub Pages from `main` when Pages
is enabled for the repository (Settings, Pages, Source: GitHub Actions); until then the deploy step is skipped and the
build is still checked. Links that leave `docs/` (the README, roadmap, source files) are rewritten to the repository on
GitHub by `scripts/mkdocs_hooks.py`; a page that is missing from `nav` in `mkdocs.yml` is appended under "More" and reported.
