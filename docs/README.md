# AgentGit documentation

| If you want to… | Read |
|---|---|
| try it in five minutes | [Getting started](getting-started.md) |
| understand the vocabulary (changeset, stream, proposal, intent…) | [Concepts](concepts.md) |
| run agents safely against a shared repository | [Agents guide](guides/agents.md) |
| call the server from TypeScript or Python | [Client SDKs](guides/sdks.md) and the [AGWP v1 reference](protocol/agwp-v1.md) |
| decide what must be true before code lands | [Policy reference](guides/policy.md) and [Validation](guides/validation.md) |
| work against a server (clone, pull, push) and offline | [Remote guide](guides/remote.md) |
| read history: blame, renames, path history | [History guide](guides/history.md) |
| watch agents work, review and integrate in a browser | [Web UI guide](guides/web-ui.md) |
| run checks on untrusted code and sign the result | [Workers guide](guides/workers.md); what the [sandbox](architecture/sandbox.md) does and does not stop |
| move to or from Git | [Git bridge](guides/git-bridge.md) |
| look up a command | [CLI reference](reference/cli.md) *(generated)* |
| understand how it works and why it is safe | [Architecture overview](architecture/overview.md) |
| check a guarantee against the code and the test that proves it | [Publication & integration](architecture/publication-and-integration.md), [Security](architecture/security.md) |
| implement a compatible reader/writer | [Object formats](architecture/object-format.md) |
| operate a repository | [Durability & backup](operations/durability.md), [GC & retention](operations/gc-and-retention.md), [PostgreSQL & S3](operations/postgres-and-s3.md), [Troubleshooting](operations/troubleshooting.md) |
| deploy and monitor the server | [Deployment](operations/deployment.md) (binary, Compose, Kubernetes), [Observability](operations/observability.md), [Releasing](operations/releasing.md) |
| talk to the server from your own code | [AGWP v1 reference](protocol/agwp-v1.md), [`openapi.json`](protocol/openapi.json) |
| see what is built and what is planned | [Roadmap](../ROADMAP.md), [Protocol overview](protocol/README.md) |
| learn why decisions were made | [ADRs](adr/) |

## Keeping the docs true

Documentation that drifts is worse than none, so most of it is *checked*:

- [`reference/cli.md`](reference/cli.md) is **generated** from `ag --help` by
  `scripts/gen_cli_docs.py`; CI fails if it is stale.
- The walkthrough in the README comes from [`examples/quickstart.sh`](../examples/quickstart.sh), which CI
  runs as a test (and so does `cargo test`).
- Byte-level format claims are backed by [`tests/conformance/`](../tests/conformance/): vectors produced by an
  independent Python implementation and verified against the Rust code.
- Every guarantee in the architecture docs names the test that exercises it.

If you change behaviour, change the doc in the same pull request.
