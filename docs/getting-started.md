# Getting started

This takes about five minutes and ends with a change landed through policy, review and validation.

## Install

AgentGit is a single Rust binary, `ag`. You need Rust **1.89+** and a C compiler (SQLite is compiled
in). `git` and `python3` are optional (Git bridge, demo script).

```bash
git clone https://github.com/Paffin/AgentGit
cd AgentGit
cargo install --path crates/ag-cli      # installs `ag` into ~/.cargo/bin
ag --version
```

Or just build it: `cargo build --release -p ag-cli` → `target/release/ag`.

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
cargo build --release -p ag-cli && ./examples/quickstart.sh
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
