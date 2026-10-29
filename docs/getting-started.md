# Get started with AgentGit

Install once, open the local demo, and make your first reviewed change. The release includes
the Web UI and all five programs; you do not need Rust, Node.js, a database server, or an API key.

This guide uses the published [v0.3.0-beta.7](https://github.com/Paffin/AgentGit/releases/tag/v0.3.0-beta.7)
for Linux and macOS. AgentGit is an early public beta. Use the demo to evaluate the workflow;
[deployment and backup instructions](operations/deployment.md) cover a real team server.

## 1. Install

Copy this into a terminal on Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/Paffin/AgentGit/main/scripts/install.sh | sh -s -- --version 0.3.0-beta.7
export PATH="$HOME/.local/bin:$PATH"
ag --version
```

The installer detects Intel/AMD or ARM, verifies the archive checksum, and installs into
`~/.local/bin` without `sudo`. If GitHub CLI is installed and signed in, it also verifies build
provenance. [Read the installer](../scripts/install.sh) or [choose another installation method](#install).

## 2. Open the demo

```sh
ag-server dev --open
```

The browser opens a signed-in demo repository, `demo/welcome`. Keep this terminal running.
No account creation or manual token setup is needed. If the browser does not open, use the
link printed in the terminal. That link signs in as you; do not share it.

The demo listens on your own machine, uses `./ag-dev-data`, and keeps its repositories when
restarted from the same directory. Stop it with **Ctrl-C**. Starting it again replaces the
short-lived demo sign-in credentials. This local evaluation mode is separate from
[production deployment](operations/deployment.md).

## 3. Make your first change

In the browser:

1. Open **Code**, then `src/greeting.py`.
2. Choose **Edit**, change `Hello` to `Hi`, and choose **Propose change**.
3. Review the diff and give the change a title. Submit it as a change request.
4. Open the change request and choose **Land**. The demo policy lets you land this simple change.

Your change is now in the `main` stream. A change request is the reviewable unit; landing it
checks policy before moving the stream. The [Git comparison](concepts-cheatsheet.md) maps
these terms to branches, commits, and pull requests you already know.

### Prefer the command line?

Open a second terminal in the directory where you started the demo, then run:

```sh
export PATH="$HOME/.local/bin:$PATH"
ag login http://127.0.0.1:8080 --token-file ./ag-dev-data/dev/token
ag clone http://127.0.0.1:8080/demo/welcome
cd welcome
```

The server also prints these commands with the actual address and absolute credential path.
Use those if you chose another port or data directory. Login saves the credential in your
user configuration; future `ag` commands use it automatically.

Edit `src/greeting.py` in your editor, then:

```sh
ag status
ag snapshot -m "A friendlier greeting"
ag push -t "A friendlier greeting"
ag remote proposals
ag remote integrate <proposal-id>
ag pull
```

Use the proposal ID printed by `ag push`; a unique prefix works too. `snapshot` records your
working directory without a staging step. `push` publishes it for review, and `integrate`
lands it if policy permits. [Remote workflow](guides/remote.md) covers the complete loop.

## 4. See what a policy protects

The demo protects `SECURITY.md`. Propose a change to that file and try to land it: AgentGit
asks for another person's approval and explains why. Your own approval does not count.

From a cloned demo repository, inspect the rules and a blocked change with:

```sh
ag remote policy main --explain
ag remote explain <proposal-id>
```

To complete the example, the demo provides a second reviewer credential at
`./ag-dev-data/dev/reviewer-token` relative to the directory where the server started.
Use that identity for a review, then return to your own identity to land the change.
[The agent-safe repository tutorial](tutorials/first-agent-safe-repo.md) walks through
approval, checks, leases, and refused changes step by step.

## Troubleshooting

| What happened | Next step |
|---|---|
| `ag: command not found` | Run `export PATH="$HOME/.local/bin:$PATH"` in this terminal, or use the exact program path printed by the installer. Add that directory to your shell's startup file if you want it in future terminals. |
| Port 8080 is in use | Start `ag-server dev --open --port 0`; it picks a free port and prints the correct login commands. |
| The browser did not open | Copy the local sign-in link from the server terminal. |
| Download discovery failed or GitHub rate-limited it | Use the version-pinned install command above. [Release downloads](https://github.com/Paffin/AgentGit/releases) are also available directly. |
| An old demo login stopped working | Use the new link or login command printed after restarting the server. |

## Install

The release archive contains `ag`, `ag-server`, `ag-worker`, `ag-mcp`, and `orchestrator`.
The Web UI is embedded in `ag-server`; there is no separate frontend server to configure.

| Platform | Available release archive |
|---|---|
| Linux with glibc | x86_64 and ARM64 |
| macOS | Intel and Apple Silicon |
| Windows | Build from source; a native release archive is not published yet |
| Alpine / musl | Use the container image or build from source |

### Installer options

The version-pinned command above is reproducible. Omitting `--version` selects the latest
stable release; while no stable release exists, it selects and identifies the latest
published prerelease. It never selects a draft release.

```sh
# Inspect the planned version, platform and destination without installing.
sh scripts/install.sh --version 0.3.0-beta.7 --dry-run

# Choose a directory you own. Binaries go into /your/path/bin.
sh scripts/install.sh --version 0.3.0-beta.7 --prefix /your/path
```

These examples assume you downloaded [install.sh](../scripts/install.sh). Its `--help` also
lists mirror and target options. Use `--require-attestation` to require a successful
`gh attestation verify` check, or `--no-attestation` to explicitly use checksum verification
alone. The installer prints a copyable command to launch the demo, even if the destination
is not on your PATH. It does not change shell profiles or start a server automatically.

### Containers

Images for Linux AMD64 and ARM64 are published as `ghcr.io/paffin/agentgit-cli` and
`ghcr.io/paffin/agentgit-server`. Use the `0.3.0-beta.7` tag for this guide. See the
[container deployment instructions](operations/deployment.md) for ports, persistent data,
credentials, and the validation worker.

### Build from source

The CLI requires Rust **1.89+** and a C compiler. SQLite is included:

```sh
git clone https://github.com/Paffin/AgentGit
cd AgentGit
cargo install --locked --path crates/ag-cli
ag --version
```

For the embedded Web UI, build the frontend before the server (Node.js **22.18+** is required here):

```sh
npm ci --prefix apps/web
npm run build --prefix apps/web
cargo install --locked --path crates/ag-server
ag-server dev --open
```

Homebrew and Scoop packaging templates are in [`packaging/`](../packaging). A maintained tap
or bucket is not published yet; do not treat those templates as an installed package source.
[Release engineering](operations/releasing.md) describes the archives and verification.

## Your first repository, without a server

Use the CLI entirely locally when you do not need shared review or a browser:

```sh
mkdir hello && cd hello
ag init --namespace me/hello
printf '# Hello\n' > README.md
ag snapshot -m "First version"
ag propose -t "Initial import"
ag integrate <proposal-id>
ag log
```

`ag propose` prints the ID; `ag proposal list` shows it again. Local work lives in `.ag/`.

## Working with a team (or with agents)

Connect with `ag login https://YOUR-SERVER`, then clone the repository URL your team gives
you. [The remote workflow](guides/remote.md) covers fetch, push, review, and offline work.
[Migrate from Git](guides/migrate-from-git.md) when you already have a repository.

## Making "landed" mean something

Choose a [policy preset](guides/policy.md#start-from-a-preset) for approvals, protected paths,
and checks. The [agent-safe repository tutorial](tutorials/first-agent-safe-repo.md) shows
a complete, tested example.

## Running agents

Start with [the agents guide](guides/agents.md). It covers the MCP server, agent identities,
scoped credentials, executor setup, tasks, and leases.

## The whole story in one script

[`examples/quickstart.sh`](../examples/quickstart.sh) demonstrates a human, two agents and a
CI service working through a conflict and policy checks. It uses an existing `ag` binary.

## Machine-readable output and exit codes

Every command accepts `--json`. Exit codes are **0** success, **1** error, **2** usage error,
**3** blocked by a requirement you can provide, and **4** refused (conflict, denial or invalid
result). Scripts should use those codes and JSON fields. See the [CLI reference](reference/cli.md).

## Where things live

Your local repository uses `.ag/` for configuration, metadata, objects and private keys.
The demo server uses `./ag-dev-data` by default. Back up local repositories with
`ag backup <dir>` and verify them with `ag verify`; [server operations](operations/deployment.md)
cover shared storage and backups. Removing the installed binaries does not remove repositories.

<a id="on-020-rc2-until-the-next-release"></a>

## Upgrading from 0.2.0-rc.2

That historical candidate did not have `ag-server dev` or saved `ag login` sessions. Use a
current release for this walkthrough. If you must maintain it, the
[rc.2 release record](operations/releasing.md#evidence) documents its limits; do not apply
newer metadata migrations without a backup and a tested upgrade path.
