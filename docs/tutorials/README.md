# Tutorials

Walkthroughs that you can follow with a terminal and nothing else. Each one builds something small and real, and each
command in it has been run by the test suite against the real `ag` (and, where a page needs it, the real `ag-server`
and `git`). Start with the first.

| Tutorial | You will | Time |
|---|---|---|
| [Your first agent-safe repository](first-agent-safe-repo.md) | write a policy, give an agent an identity, a lease and a workspace, watch its change be refused, then provide the CI result, provenance and review that let it land; let a documentation change land with no human | about 15 minutes |
| [Run a team server](team-server.md) | start `ag-server`, create a repository, give four identities different roles, push, review and integrate over the network, and watch the server refuse what roles do not allow | about 15 minutes |
| [Policy cookbook](policy-cookbook.md) | pick a recipe (solo, CI plus review, four eyes on sensitive paths, agents on docs, merge queue, release stream, trusted workers) and see the mistakes the parser catches | reference, 5 minutes per recipe |
| [Migrating a Git repository](migrating-from-git.md) | import a Git history, prove nothing was lost by exporting it again and comparing trees, turn on policy, and run both side by side | about 10 minutes |
| [Troubleshooting a blocked integration](troubleshooting-blocked-integration.md) | read every refusal `ag integrate` can print, reproduce each one, and fix it | reference |

The times are how long it takes to read and follow along, not how long the commands run (a few seconds each page).
After the tutorials: [Concepts](../concepts.md), the [FAQ](../faq.md), the [glossary](../glossary.md), and
[AgentGit compared](../comparison.md) with Git, GitHub, Gerrit and Jujutsu.

## How these tutorials are tested

A tutorial that stops working is worse than none, so these pages are executable documentation.

- **Every shell block is run.** `scripts/doc_snippets.py` executes the blocks of a page in order, in one scratch directory
  with its own `HOME`, in a single bash process (so `cd`, `export` and variables carry over, as in a terminal), with the
  `ag` and `ag-server` binaries that CI just built. Nothing is mocked: the server in the team tutorial is the real
  `ag-server`, the "agents" run the real commands, the Git repository in the migration tutorial is a real Git repository.
- **Every command's exit status is checked.** A statement must exit 0 unless it ends in `# expect-exit N`, and the first
  one that does not stops the page. That is how the pages prove that an integration really exits 3 when it is blocked and
  4 when it is refused.
- **The output you are shown is checked.** A text block after a command (`<!-- expect -->` in the source) must appear in
  what the command printed; `...` stands for text that changes from run to run, such as ids and timestamps.
- **A page cannot quietly stop being tested.** A page that has any marker must mark every shell block as either run
  (`<!-- doctest -->`) or skipped with a reason (`<!-- doctest-skip: why -->`).
- **The policy documents are parsed by the real parser** (`cargo test -p agentgit-policy --test cookbook`), and the recipes
  marked *run* in the cookbook are stored in a real repository by the harness.
- **The refusal messages are tied to the code.** The table in the troubleshooting tutorial is checked against the Rust
  sources by `scripts/check_docs.py`.
- **Where it runs.** The `Docs` workflow (`.github/workflows/docs.yml`) builds the binaries and runs the harness on every
  push and pull request; `cargo test -p agentgit-cli` runs the pages that need only `ag`. To run them yourself:
  `cargo build -p agentgit-cli -p agentgit-server && python3 scripts/doc_snippets.py` (add `--show` to see every statement with its
  output, `--list` to see what would run).

What this does **not** prove: that the commands are what you *should* run in your situation, how they behave on
Windows (the harness needs bash, so the pages run on Linux and macOS only), or anything about performance. The one
port the team-server page uses (`7878`) is replaced by a free one while it runs.
