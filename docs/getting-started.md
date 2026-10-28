# Getting started

Five minutes, and nothing to build: **no Rust, no Node, no tokens to make by hand.** You download the programs, start an
evaluation server that sets itself up, sign in, land a first change, and see a policy say no and tell you why.

> **Version.** This guide targets the published [`v0.3.0-beta.5` candidate](https://github.com/Paffin/AgentGit/releases/tag/v0.3.0-beta.5), which includes `ag-server dev`, `ag login`,
> `ag explain` and policy presets. If you still use the previous candidate, use
> [the 0.2.0-rc.2 procedure](#on-020-rc2-until-the-next-release). This is an early public beta; independent production qualification is still pending.

## 1. Get the programs

A release archive for Linux or macOS (x86_64 and aarch64). The script checks the checksum before it installs anything and
puts `ag`, `ag-server`, `ag-worker` and `ag-mcp` in `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/Paffin/AgentGit/main/scripts/install.sh | sh -s -- --version 0.3.0-beta.5   # use the candidate version
ag --version
```

(No stable release exists yet, so `latest` skips pre-releases and the script needs `--version`. More ways, including
the container images, Homebrew and building from source, are under [Install](#install).)

## 2. Start the evaluation server

```bash
ag-server dev
```

```text
AgentGit dev server: for trying AgentGit out, not for real work
  - loopback only (127.0.0.1), plain HTTP; do not expose this port
  - one administrator (user:dev); its credential lives 8 hours, is replaced at every start and is kept in /home/you/ag-dev-data/dev/token
  - the demo repository demo/welcome has the `agents` policy preset (`ag remote policy main --explain` says what it means); data in /home/you/ag-dev-data

Open in a browser (signs you in; the link carries the credential, so do not share it):
  http://127.0.0.1:8080/repos/demo/welcome#token=agt_…

A second user, user:reviewer, can approve what user:dev proposes (nobody's own approval counts); its credential is in /home/you/ag-dev-data/dev/reviewer-token.

Command line:
  ag login http://127.0.0.1:8080 --token-file /home/you/ag-dev-data/dev/token
  ag clone http://127.0.0.1:8080/demo/welcome

Credential expires 2026-10-21T10:16:50.403Z. Stop with Ctrl-C; start again with the same --data-dir to keep everything.
```

That one command made the data directory `./ag-dev-data` (delete it to start over), an administrator with a fresh
credential, a demo repository with four sample files, and the `agents` [policy preset](guides/policy.md#start-from-a-preset).
Start it again with the same directory and your repository, history and any policy changes stay; only the credential is replaced, and the old one stops working.
Options: `--port N` (`0` picks a free one), `--data-dir DIR`, `--open` (opens the browser for you).

**What differs from a real deployment**, and why you should not keep anything you care about here: it listens on
`127.0.0.1` only and speaks plain HTTP (it refuses another address unless you add `--allow-non-loopback`); its single
administrator credential lives 8 hours, where a deployment's default is 24 hours and its cap 30 days, and changes at every start; the
policy is a fixed preset; there is no single sign-on, no TLS, no backups. For a real server see [Deploying `ag-server`](operations/deployment.md).

*Containers (not run here: this machine has no Docker daemon).* In a container the server has to listen on all of its own
interfaces, so publish the port to your loopback only:
`docker run --rm -p 127.0.0.1:8080:8080 ghcr.io/paffin/agentgit-server:<a version that has `dev`> dev --data-dir /data --host 0.0.0.0 --allow-non-loopback`.
The browser link works as printed; for the command line, fetch the credential from the container's `/data/dev/token`
(`docker cp CONTAINER:/data/dev/token -` writes it as a tar stream) and pass it to `ag login --token-stdin`.

## 3. Sign in

**In the browser**, open the link from the banner. It signs you in and opens the demo repository's live board. The
credential is in the part of the address after `#`, which a browser never sends to a server; the page removes it from the
address bar at once. The trade-offs are written down in [The Web UI](guides/web-ui.md#signing-in).

**On the command line**, run the two commands from the banner:

```bash
ag login http://127.0.0.1:8080 --token-file ~/ag-dev-data/dev/token
ag clone http://127.0.0.1:8080/demo/welcome
cd welcome
```

```text
Logged in to http://127.0.0.1:8080 as user:dev (the credential expires 2026-10-21T10:16:50.403Z).
Saved in ~/.config/agentgit/credentials.json (readable by you only); `ag clone`, `ag fetch`, `ag push` and `ag remote` to this server use it.
Cloned demo/welcome into welcome (1 changeset(s), 4 blob(s)); stream 'main' at acdaaa5b59af.
```

`ag login` asks the server who the credential belongs to before it saves anything, and the saved file is yours alone
(mode 0600). From now on `ag clone`, `ag fetch`, `ag push` and `ag remote …` to this server need no token. A variable such as
`AG_TOKEN` still takes precedence, so a script can say which credential it wants. `ag logout URL` forgets it.

## 4. Your first change

```bash
sed -i.bak 's/Hello/Hi/' src/greeting.py && rm src/greeting.py.bak    # or edit it with any editor
ag status                                  # shows: modified  src/greeting.py
ag snapshot -m "A friendlier greeting"     # record the working directory (no staging area)
ag push -t "A friendlier greeting"         # publish it and open a proposal on the server
ag remote proposals                        # the id is the first column; any unique prefix works
ag remote integrate prop_90168b            # the policy decides; here it lets a person land their own change
ag pull
```

```text
Snapshot fe468b68f68b in workspace 'default'
Uploaded 2 object(s) and opened proposal prop_90168b6ce4176298a007 -> 'main'.
Integrated (fast_forward): stream is now at fe468b68f68b (version 2), 1 file(s) changed.
```

You made a **snapshot** (a private record of your directory), **published** it as a **proposal**, and the server
**integrated** it: it checked the **policy** and moved the stream `main` with a compare-and-swap. Refresh the live board in the
browser and the change is there. [Concepts](concepts.md) has the vocabulary; [Git to AgentGit](concepts-cheatsheet.md) maps it
from what you already know.

## 5. See the policy say no, and why

The demo policy protects `SECURITY.md`: a change to it needs an approval from somebody other than its author.

```bash
echo "Report problems to security@example.org." >> SECURITY.md
ag snapshot -m "Contact address"
ag push -t "Contact address"
ag remote integrate prop_05ff36       # exit status 3: blocked (the id from `ag push`, or the newest in `ag remote proposals`)
```

```text
Blocked (policy v1); the stream was not changed:
  - approvals (SECURITY.md): 0 of 1 required
Provide what is missing, then run `ag remote integrate prop_05ff36` again.
  ag remote review prop_05ff36 --approve
`ag remote prepare prop_05ff36` shows what is still missing.
```

Nobody's own approval counts, so the demo made a second user for you. Review as her, then land it as yourself:

```bash
AG_TOKEN=$(cat ~/ag-dev-data/dev/reviewer-token) ag remote review prop_05ff36 --approve
ag remote integrate prop_05ff36
```

Exit status **3** means *blocked*: something you can provide is missing, and the output names the command that provides it.
**4** means *refused*: a conflict or a denial. Scripts and agents branch on those, not on the text.

What the policy allows and refuses, in plain words, read from the policy document itself:

```bash
ag remote policy main --explain
```

For a proposal in a repository of your own, `ag explain [PROPOSAL]` says why it does not land yet and lists the exact
commands that fix each reason (a check to run, a review to ask for, a lease to take, a conflict to resolve).

## Where to go next

- **Let an agent in.** `ag agent register`, `ag agent start`, and the autonomy envelope the `agents` preset sets: [Running agents](guides/agents.md), and
  [Your first agent-safe repository](tutorials/first-agent-safe-repo.md) (its commands are run by the tests).
- **Choose a policy.** `ag policy preset list` shows `solo`, `team`, `agents` and `strict`; [Policy](guides/policy.md#start-from-a-preset).
- **Come from Git.** [Git to AgentGit](concepts-cheatsheet.md) and [Migrating from Git](tutorials/migrating-from-git.md).
- **Run it for a team.** [Deploying `ag-server`](operations/deployment.md) and the [team-server tutorial](tutorials/team-server.md).

## On 0.2.0-rc.2 (until the next release)

The pre-release has no `dev` command, so the server needs three steps of its own. These were run against the published
`v0.2.0-rc.2` Linux archive (checksum verified by `install.sh`):

```bash
export AG_BOOTSTRAP_TOKEN=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')   # the administrator secret, from the environment
ag-server admin --data-dir ./ag-data create-repo demo/welcome
ag-server serve --data-dir ./ag-data &                                           # 127.0.0.1:8080
export AG_TOKEN=$AG_BOOTSTRAP_TOKEN                                              # every ag command below uses it
ag clone http://127.0.0.1:8080/demo/welcome && cd welcome
echo "# welcome" > README.md
ag snapshot -m "first" && ag push -t "First change"
ag remote integrate prop_…           # there is no policy yet, so a person's change lands
```

In the browser, open `http://127.0.0.1:8080/` and paste `$AG_BOOTSTRAP_TOKEN` as the access token. The bootstrap token is
all-powerful; use it only to make real credentials (`ag-server admin mint`), then unset it.
There is no `ag login` in rc.2: keep `AG_TOKEN` set, or use `--token-stdin` with `ag clone`. Stop the server with `kill %1` (or Ctrl-C if it runs in its own terminal).

## Install

AgentGit ships three programs: `ag` (the command line), `ag-server` (the network server, with `ag-server dev` for trying it
out) and `ag-worker` (runs validation jobs), and `ag-mcp` for agents. Pick the way that suits you:

> **Building from source always works.** What was verified about the release files is in
> [Releasing](operations/releasing.md#evidence).

**1. Release archive (Linux and macOS, x86_64 and aarch64)** — a script that checks what it downloads:

```bash
curl -fsSL https://raw.githubusercontent.com/Paffin/AgentGit/main/scripts/install.sh | sh -s -- --version 0.3.0-beta.5   # no stable release yet: `latest` skips pre-releases
ag --version                              # ag 0.3.0-beta.2 (549555c82219 2026-10-27): version, commit, commit date
```

It works out your platform, downloads `agentgit-<version>-<target>.tar.gz` and `SHA256SUMS`, refuses to install
anything if the checksum differs, additionally verifies the build-provenance attestation when the GitHub CLI is
installed and logged in, and installs `ag`, `ag-server`, `ag-worker` and `ag-mcp` into `~/.local/bin` (no root). Options:
`--version X.Y.Z`, `--prefix DIR`, `--dry-run` (print what it would do), `--require-attestation`. Read it first
if you like: [`scripts/install.sh`](../scripts/install.sh) is short and `shellcheck`-clean. The Linux binaries
need glibc (Alpine/musl is not supported; use the container image or build from source). Doing it by hand:

```bash
V=0.3.0-beta.2; T=x86_64-unknown-linux-gnu          # or aarch64-unknown-linux-gnu, aarch64-apple-darwin, x86_64-apple-darwin
curl -fsSLO https://github.com/Paffin/AgentGit/releases/download/v$V/agentgit-$V-$T.tar.gz
curl -fsSLO https://github.com/Paffin/AgentGit/releases/download/v$V/SHA256SUMS
sha256sum -c SHA256SUMS --ignore-missing     # macOS: shasum -a 256 -c SHA256SUMS --ignore-missing
gh attestation verify agentgit-$V-$T.tar.gz --repo Paffin/AgentGit     # optional, needs gh
tar xzf agentgit-$V-$T.tar.gz && install agentgit-$V-$T/ag* ~/.local/bin/
```

**2. Container images** (`linux/amd64` and `linux/arm64`, distroless, non-root):
`docker run --rm ghcr.io/paffin/agentgit-cli:0.3.0-beta.5 --version`, and
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

## Your first repository, without a server

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

By default anyone who is not an agent can integrate. Pick a [preset](guides/policy.md#start-from-a-preset) to require
reviews, or reviews and a signed check, and read back what it means:

```bash
ag policy preset list                       # solo, team, agents, strict
ag policy preset apply team --ci-key ci    # one approval from somebody else, and a signed check named "tests"
ag policy explain                           # the policy in plain words, read from the document itself

ag integrate <proposal>          # exit code 3: "check 'tests' has no trusted passing attestation…"
ag explain <proposal>            # why, and the exact commands that fix each reason
ag validate run <proposal> --check tests --key ci -- cargo test
ag --as user:bob proposal review <proposal> --approve     # somebody else: your own approval does not count
ag integrate <proposal>          # lands; the attestation names the exact tree that was checked
```

A preset is an ordinary policy document, so you can `ag policy preset show team > policy.json`, edit it and
`ag policy set policy.json`; `ag init --preset team` chooses one when the repository is made. Or write one by hand:
`ag policy example > policy.json`.

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
