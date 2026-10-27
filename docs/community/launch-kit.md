# Pilot launch kit

These are drafts for maintainers to publish after checking the named version and evidence. No outreach or independent user results are implied by this document.

## Pilot invitation

> Do you use multiple coding agents on the same repository? We are testing AgentGit, an open-source source-control system that binds checks to the exact code being integrated and explains why a change is blocked. We are looking for five small teams to try one real task on a non-critical repository, then repeat the workflow a week later. You can export code back to Git. The project is alpha; the supported scope and limitations are public. Tell us your operating system, agent host and the integration problem you want to solve in a pilot report.

Link to the [beta programme](beta.md) and a published candidate. Do not send an invitation that points at unbuilt `main` instructions.

## Three-minute demonstration

1. **0:00–0:30 — the task:** show a real repository, two agents and the intended change. Name the version and platform.
2. **0:30–1:15 — stale evidence:** validate a proposal, move the target stream with another change, then show why the previous validation cannot authorize the new candidate.
3. **1:15–2:00 — recovery:** run the required validation, obtain the eligible review and integrate. Show the changed files and the four independent verdicts.
4. **2:00–2:30 — boundaries:** attempt an agent change outside its allowed scope and show the refusal. Explain the human action needed.
5. **2:30–3:00 — exit path:** export to Git, identify what the export does not contain, link to backup/restore and invite a pilot report.

Record the terminal and browser against a real server. Keep failed attempts if they explain the workflow. Caption the video; publish commands and expected exit codes alongside it. Do not use simulated passing checks or imply that model output is guaranteed correct.

## Technical post outline

Title: **What happens when two coding agents invalidate each other's checks?**

Start with the concrete race, show a minimal reproduction, explain exact-tree validation and the policy decision, then compare the relevant workflow fairly with existing merge queues. Include measurements only with their method and environment. End with installation, limitations and the pilot invitation. Submit to appropriate coding-agent/MCP communities and Show HN only after the reproduction works from the public release; follow each community's rules.

## Case-study template

- Participant permission and attribution preference.
- Repository type, team size, agent host, operating system and AgentGit version.
- Original problem and workflow.
- Commands and reproducible task.
- First integration time, interventions and failures.
- Result after one week: repeated use, abandonment or unresolved blocker.
- What AgentGit helped with and what it did not solve.

Do not substitute internal role-play or a maintainer-run session for an external pilot.
