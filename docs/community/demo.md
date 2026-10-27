# Watch the exact-tree workflow

This three-minute recording uses the [published rc.3 archive](https://github.com/Paffin/AgentGit/releases/tag/v0.2.0-rc.3), a real server and a real Linux namespace worker. A separate reviewer credential approves the proposals. One operator controls both scripted workspaces and the reviewer: this is a reproducible demonstration, not a live model run or an independent pilot.

<video controls preload="metadata" style="width:100%;height:auto" poster="../assets/exact-tree-demo.png" aria-label="A passing validation becomes stale after the target stream moves">
  <source src="../assets/exact-tree-demo.webm" type="video/webm">
  Download the video using the link below.
</video>

[Download the captioned video](../assets/exact-tree-demo.webm) · [Read the transcript and commands](../assets/exact-tree-transcript.txt) · [Download the recorder](../assets/reproduce-exact-tree.mjs)

## What happens

1. Workspace A changes a file, opens a proposal and receives a review. The worker checks the candidate and signs a passing result.
2. Workspace B changes a different file, receives a review and passes its check, then integrates.
3. The target stream has moved. A's earlier passing check does not authorize its new combined candidate: `ag remote explain` returns **3 (blocked)**.
4. The worker checks A's new candidate. `ag remote explain` returns **0 (ready)**, and the integration succeeds.
5. The operator pulls and exports to Git, then verifies that the concurrent file is present. Git export does not preserve AgentGit reviews, policy, attestations or audit; those need a backup.

The check deliberately tests only whether two files exist. It demonstrates binding a real result to the exact candidate tree; it is not evidence that a real application is correct or that model output is reliable.

## Reproduce it

Use Linux with the namespace sandbox available, Node.js 22.18 or newer, npm and Git. `ag-worker doctor` must report that the namespace sandbox works. Download and unpack rc.3, then clone this repository and install the browser dependency:

```bash
npm --prefix apps/web ci
(cd apps/web && npx playwright install chromium)
AG_SOURCE_ROOT="$PWD" AG_BIN_DIR=/absolute/path/to/unpacked/rc3 \
  node docs/assets/reproduce-exact-tree.mjs
```

The recorder starts a loopback development server, creates temporary credentials and stops on an unexpected exit code. No provider API key or production credential is needed. It writes the video and assertions to `agentgit-demo/` in the current directory. Full commands and expected exits are in the transcript.

## Release verification and limits

[Linux verification record](../assets/rc3-linux-verification.json) records a separate run against the publicly downloaded archive: onboarding, protected-path review, backup/restore and an upgrade from rc.2 preserving policy, a reviewed proposal, the stream head and ten audit entries. The archive checksum matches the published `SHA256SUMS`. Local provenance-bundle retrieval returned HTTP 403; [the release workflow](https://github.com/Paffin/AgentGit/actions/runs/40470846262) separately verifies attestations and installs the archives on Linux x86_64, Linux ARM64 and macOS.

These checks do not replace [the external pilot and platform gates](beta.md). The project remains alpha.

## Additional rc.3 qualification

These are repeatable maintainer checks on Linux x86_64 with SQLite and local object storage, not independent customer trials:

- [Live backup record](../assets/rc3-live-backup.json): 80 acknowledged writes and six independently verified backups. Writes completed during 1 of the backup windows. Every restored counter/event sequence was coherent; the pending review and policy were preserved. [Reproducer](../../examples/qualify-backup.py).
- [Restart and replay record](../assets/rc3-restart-replay.json): the client loses the integration response, the real server is killed, and an identical-key retry after restart returns the original result without another stream update. This checks an unknown outcome after commit, not hardware power loss. [Reproducer](../../examples/qualify-restart.py).
- [Git adapter record](../assets/rc3-git-exit.json): import, an AgentGit change and export preserve the compared Git trees and history structure. The fixture includes an executable, a symlink and an LFS pointer. Tags are not exported; a gitlink is reported and skipped; LFS payloads are not fetched. [Reproducer](../../examples/qualify-git-exit.py).

Set `AG_BIN_DIR` to the directory containing the unpacked published binaries and run each Python script. They need Python 3.9+, Git and roughly 500 MB of temporary disk space for the backup fixture. Temporary fixtures are removed on exit; JSON evidence is written to the current directory (or `AG_EVIDENCE_DIR`).

The engine's deterministic `crash_at_every_step_recovers_to_exactly_one_integration` and `idempotent_replay_and_unknown_outcome_recovery` tests also pass. They inject failures at seven publication/integration checkpoints on test stores; they do not simulate a physical disk or a power failure.
