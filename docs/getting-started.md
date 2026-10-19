# Getting started

This takes about five minutes and ends with a change landed through policy, review and validation.

## Install

AgentGit ships three programs: `ag` (the command line, all you need to start), `ag-server` (the network
server) and `ag-worker` (runs validation jobs). Pick the way that suits you:

> **Status.** No release has been published yet: the first `v*` tag will publish everything below. Until then
> the release pipeline has only been run as a *rehearsal* (it builds, checks and installs the real archives but
> publishes nothing — see [Releasing](operations/releasing.md)), so the installer and the container images
> have not been used against a real release. **Building from source always works.**

**1. Release archive (Linux and macOS, x86_64 and aarch64)** — a script that checks what it downloads:

```bash
curl -fsSL https://raw.githubusercontent.com/Paffin/AgentGit/main/scripts/install.sh | sh -s -- --version 0.2.0-rc.1   # no stable release yet: `latest` skips pre-releases
ag --version                              # ag 0.2.0-rc.1 (a1b2c3d4e5f6 2026-10-19): version, commit, commit date
```

It works out your platform, downloads `agentgit-<version>-<target>.tar.gz` and `SHA256SUMS`, refuses to install
anything if the checksum differs, additionally verifies the build-provenance attestation when the GitHub CLI is
installed and logged in, and installs `ag`, `ag-server`, `ag-worker` and `ag-mcp` into `~/.local/bin` (no root). Options:
`--version X.Y.Z`, `--prefix DIR`, `--dry-run` (print what it would do), `--require-attestation`. Read it first
if you like: [`scripts/install.sh`](../scripts/install.sh) is short and `shellcheck`-clean. The Linux binaries
need glibc (Alpine/musl is not supported; use the container image or build from source). Doing it by hand:

```bash
V=0.2.0-rc.1; T=x86_64-unknown-linux-gnu          # or aarch64-unknown-linux-gnu, aarch64-apple-darwin, x86_64-apple-darwin
curl -fsSLO https://github.com/Paffin/AgentGit/releases/download/v$V/agentgit-$V-$T.tar.gz
curl -fsSLO https://github.com/Paffin/AgentGit/releases/download/v$V/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing     # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify agentgit-$V-$T.tar.gz --repo Paffin/AgentGit     # optional, needs gh
tar xzf agentgit-$V-$T.tar.gz && install agentgit-$V-$T/ag* ~/.local/bin/
```

**2. Container images** (`linux/amd64` and `linux/arm64`, distroless, non-root):
`docker run --rm ghcr.io/paffin/agentgit-cli:0.2.0-rc.1 --version`, and
`ghcr.io/paffin/agentgit-server` for the server — see [Deploying `ag-server`](operations/deployment.md).

**3. Homebrew / Scoop.** The repository carries a formula and a manifest *template* under
[`packaging/`](../packaging) and `scripts/render_packaging.py` fills them from a release's `SHA256SUMS` (every
release attaches the rendered `agentgit.rb`). There is no tap and no Scoop bucket yet, and the release workflow does not build
a Windows archive yet (the Windows port is tested by its own workflow), so treat both as unpublished.

**4. From source.** You need Rust **1.89+** and a C compiler (SQLite is compiled in). `git` and `python3` are
optional (Git bridge, demo script).

```bash
git clone https://github.com/Paffin/AgentGit
cd AgentGit
cargo install --path crates/ag-cli      # installs `ag` into ~/.cargo/bin
cargo install --path crates/ag-server   # optional: the server
ag --version
```

Or just build it: `cargo build --release -p agentgit-cli` → `target/release/ag`. (The crates are not on crates.io yet. Their packages are named `agentgit-*`, so Cargo's `-p` flag says
`-p agentgit-cli`, while the directories (`crates/ag-cli`), binaries and library names keep the short `ag` names;
[Releasing](operations/releasing.md#cratesio) has the scheme and the publish order.)

## Your first repository

```bash
mkdir hello && cd hello
export AG_IDENTITY=user:alice            # who you are; defaults to user:$USER
ag init --namespace me/hello             # creates .ag/ and the stream "main"

echo "# hello" > README.md
ag status                                # added README.md
ag snapshot -m "first"                   # record the working directory (a *snapshot*)
ag propose -t "Initial import"           # publish it and open a proposal
ag integrate <proposal-id>               # move the stream "main" to include it
ag log
```

Proposal ids are long; any unique prefix of at least six characters works
(`ag integrate prop_8f3a`). `ag proposal list` shows them.

What just happened, in AgentGit terms:

1. `snapshot` stored your files as immutable *blobs* and a *changeset* in your **workspace**. Nothing is
   visible to anyone else yet.
2. `propose` **published** the changeset — every object was re-read and verified, then registered in a
   single transaction — and opened a **proposal** targeting the stream `main`.
3. `integrate` computed the result (a fast-forward here, a deterministic merge in general), checked
   **policy**, and atomically moved the head of `main` with a compare-and-swap.

Read [Concepts](concepts.md) for the full vocabulary.

## Working with a team (or with agents)

```bash
ag workspace create alice-fix --dir ../alice-fix   # an isolated line of work, in its own directory
cd ../alice-fix && $EDITOR src/lib.rs
ag snapshot -m "fix off-by-one" --workspace alice-fix
ag propose @alice-fix -t "Fix off-by-one"
ag preview <proposal>          # what would integration do? conflicts? (changes nothing)
ag integrate <proposal>
ag sync --workspace alice-fix  # bring the stream's new head into the workspace
```

If two people edit the same lines, `integrate` reports a **conflict** (exit code 4) and leaves the
stream untouched. Run `ag sync`: the clean parts are applied and conflicting files get
`<<<<<<< ours` markers. Resolve them, `ag snapshot`, `ag proposal update <id> @workspace`, integrate again.

## Making "landed" mean something

By default anyone can integrate. Add a policy to require reviews and validation:

```bash
ag validate keygen ci            # a signing key for your CI; prints the public key to trust
ag policy example > policy.json  # edit: trusted_issuers, required_validations, approvals…
ag policy set policy.json

ag integrate <proposal>          # exit code 3: "check 'tests' has no trusted passing attestation…"
ag validate run <proposal> --check tests --key ci -- cargo test
ag proposal review <proposal> --approve
ag integrate <proposal>          # lands; the attestation names the exact tree that was checked
```

Details: [Policy reference](guides/policy.md), [Validation](guides/validation.md).

A longer version of this, with an agent, a lease and a refused integration at every step, is
[Your first agent-safe repository](tutorials/first-agent-safe-repo.md); its commands are run by the tests.

## Running agents

```bash
ag agent register refactor-bot          # identity + signing key
ag agent start refactor-bot             # prints: act as agent:refactor-bot#run_…
export AG_IDENTITY=agent:refactor-bot#run_…
ag lease acquire 'src/**' --exclusive   # prints a fencing token
```

See [Agents guide](guides/agents.md) for the full loop (tasks, leases, provenance, autonomy limits).

## The whole story in one script

[`examples/quickstart.sh`](../examples/quickstart.sh) runs a human, two agents and a CI service through
policy, a blocked integration, a lease, a conflict and its resolution. Run it and read its output:

```bash
cargo build --release -p agentgit-cli && ./examples/quickstart.sh
```

## Machine-readable output and exit codes

Every command accepts `--json`. Exit codes are stable: `0` ok, `1` error, `2` usage error,
`3` blocked (something you can provide is missing), `4` refused (conflict / denied / invalid).
Scripts and agents should branch on these, not on message text. See the
[CLI reference](reference/cli.md).

## Where things live

```text
hello/
├── README.md             your files
└── .ag/
    ├── config.json       namespace, default stream
    ├── meta.db           SQLite: index, streams, reflog, audit, policy…  (+ -wal/-shm while open)
    ├── store/objects/    immutable, content-addressed blobs and changesets
    └── keys/             agent and issuer private keys (0600) — keep out of untrusted environments
```

Back it up with `ag backup <dir>`; check it with `ag verify`.
