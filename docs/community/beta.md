# Beta programme

AgentGit offers an early public beta for small teams working with coding agents. The goal is to let a team review and land agent changes with checks tied to the exact candidate tree and a clear explanation when policy blocks a change.

**Current status: early public beta (`0.3.0-beta.2`).** This opens the supported single-server workflow to early adopters; it does not claim independent pilot results or production readiness. The remaining qualification gates below stay visible.

## Supported beta target

| Surface | Beta target | Outside the initial commitment |
|---|---|---|
| Server | One Linux server, SQLite and local durable object storage | HA, hostile multi-tenant hosting |
| Clients | Linux and macOS CLI, Web UI | Windows release archives until separately verified |
| Validation | Linux worker in a documented supported sandbox | Native macOS/Windows isolation |
| Agents | One real MCP host with a recorded end-to-end run | Assuming every MCP host works from a configuration example |
| Recovery | Verified backup/restore and Git export | Treating a Git mirror as a backup of policy, reviews or audit |
| Storage | Local filesystem | PostgreSQL/S3 qualification remains a separate track |

This is the beta support scope, not a claim that every external qualification gate has passed. See [the readiness checklist](#release-gates).

## Join a pilot

Open a [pilot report](https://github.com/Paffin/AgentGit/issues/new?template=pilot_report.yml). Use a non-critical repository with a Git mirror and a backup. Please do not attach tokens, private source, customer records or credentials.

We are looking for 5–10 developers or small teams who already use coding agents and can try one real task, then return to the workflow the following week. Installation failures and confusing terminology are useful results too.

The first session should answer: can you install a published version, open the UI, run a task, understand a blocked proposal, review it, land it and export the resulting code to Git without a maintainer telling you each step?

## Release gates

Keep each gate open until its evidence is linked. A maintainer records version, commit, platform, command or scenario, expected outcome, actual outcome and artifact. A skipped test is not a pass.

- [ ] Published candidate includes the quickstart commands shown in its versioned documentation.
- [ ] Required CI checks pass on the release commit; surviving policy mutations are resolved or individually justified as equivalent.
- [ ] Install and uninstall boundaries are verified on the supported platform matrix; archive checksums and provenance verified.
- [ ] Restart, interrupted integration, replay and concurrent writers preserve acknowledged changes.
- [ ] Backup, restore and `ag verify` succeed on the supported server configuration.
- [ ] Upgrade from the previous published release preserves files, heads, policy, proposals and audit; incompatible downgrade is documented.
- [ ] A real MCP host completes a task-to-proposal workflow with scoped credentials; denied operations remain denied.
- [ ] Four of five first-time users finish the published quickstart within 15 minutes without coaching.
- [ ] At least five external pilots complete a task; at least three return in the second week.
- [ ] Two reproducible pilot case studies are published with limitations and permission from participants.
- [ ] No unresolved critical data-loss, policy-bypass or recovery defect in the supported scope.
- [ ] Release notes, known limitations, support route and compatibility policy are published.

## Measurement

Record install attempts, successful first integration, elapsed time, first blocker, second-week return and accepted contributions. Report denominators (for example 4/5), not just percentages. Collect this through voluntary reports and interviews; no background product telemetry is required.

Targets: 5–10 pilots, 80% unassisted first integration within 15 minutes, three second-week returns, two case studies and three external contributors. These are goals, not observed results or service guarantees.

## Release sequence

1. Ship a candidate that makes installation and documentation agree.
2. Qualify the narrow supported workflow and fix repeated pilot blockers.
3. Publish early beta with explicit scope and open qualification gates; require the full evidence set before a production-ready release.
4. Expand storage, host and operating-system support through separate qualification reports.

Keep the existing alpha release available. Publish candidates as prereleases. Do not rename a tag or silently replace an archive. Changes to stored data need a documented upgrade path; backup before upgrading.

## Operating the pilot

The maintainer triages reports twice a week and aims for a first response within two working days; this is an operating target, not a paid SLA. Classify each report as installation, workflow, correctness, recovery, accessibility or documentation. Link duplicates, give the next action and update the reporter when a fix is available. Report vulnerabilities privately through [SECURITY.md](../../SECURITY.md).
