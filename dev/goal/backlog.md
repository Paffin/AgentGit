# Ordered platform backlog

Reference: [objective](../../doc/goal.md), [full matrix](coverage.csv), [evidence](evidence.md).
All original 142 matrix rows remain in scope; the added search, scheduling and run-inspection rows bring the inventory to 150 rows. This ordering is a sequence of verifiable slices, not a reduced definition of readiness.

| Stage | Outcome and acceptance | Current state |
| --- | --- | --- |
| P0 editor ownership | A browser-created edit submits once, retains its original session across retries, preserves the draft after cancellation, and cannot redirect a different session. Prove both a clean-server UI path and negative transport cases. | Merged in PR74; targeted real-browser checks and hosted gates passed. |
| P0 confidential-draft boundary | Define who can read draft metadata and referenced objects before adding account-backed autosave. Test owner, other reader, unrelated project and admin; document compatibility. | Open. Workspace read/list is currently repository-readable. |
| P1 code search | Find literal content at a revision, continue a bounded scan and open the pinned matching line. | Merged in PR78; HTTP, gRPC, real-browser qualification and hosted checks passed. Exclusions are explicit; indexed/global/regex search remains open. |
| P1 multi-file editing | Edit, add, rename and delete several files in one browser change request; preview the exact combined diff, preserve base/modes and recover the batch. No CLI preparation counts. | Basic editor merged in PR75 and verified in two browser profiles; broader role, large-batch and device qualification remains. |
| P1 review conversations | Durable line threads, resolution, suggestions and requested reviewers tied to an inspected head; stale approval remains invalid. | Author/reviewer discussion is implemented with separate comment permission, head binding, draft recovery and idempotent writes. Line threads, resolution, suggestions and requested reviewers remain open. |
| P1 task navigation | Permanent task pages, prerequisite/dependant links and confirmed task completion/cancellation over real APIs. | Implemented and locally verified in Chromium and phone viewport; issue-to-task handoff now creates and links work atomically; browser executor configuration and final saved output are implemented; live-model and independent-pilot qualification remain open. |
| P1 user issues | Repository-scoped issues, durable discussion, close/reopen, optimistic edits and task links. | Implemented on main; focused HTTP lifecycle and real Chromium browser scenario passed. PostgreSQL persistence/reopen is qualified; broader qualification remains. Labels, milestones, attachments, boards and provider issue import remain open. |
| P1 identity and tracker | Organization/member/invitation lifecycle and user issues connected to orchestrator tasks, with role checks and history. | Open; see individual matrix rows. |
| P1 execution control | Operators pause/resume new task runs with durable state, a reason and server-side enforcement; existing runs keep their lifecycle. | Implemented on main. Browser and real command scheduler qualification passed; external-model qualification and host installation remain separate. |
| P1 run inspection | Durable run pages with current status, summary, task and exact-session change links. | Implemented and locally qualified; final stdout/stderr storage and browser executor setup are implemented; live streaming remains open. |
| P2 execution | Configure a real executor from UI, run a DAG, retain logs/artifacts, cancel and safely retry with bounded resources. | Open; scripted stand-ins do not qualify this stage. |
| P2 administration and portability | Browser setup/diagnostics, verified restore, hosted-data migration, release/registry/webhook lifecycle. | Open; assess each subsystem separately. |
| External qualification | Real MCP host, clean macOS, physical device, human screen reader, independent pilots, comparable pinned-competitor benchmark. | Open issues #50, #51, #52, #53 and #55; unavailable equipment/participants are not replaced by simulation. |

Next product priority: finish the browser path from a human request to bounded agent execution, then inspect the exact candidate checks, review and integration result. The issue tracker is the intake and traceability layer, not the defining product. Provider migration supports adoption of this workflow. Do not substitute issue-tracker breadth or CI maintenance for agent-native execution controls.

Baseline for this resumed work: main `549555c82219c6465e59b4d16a3d5a9d3bea47f9`, 2026-10-28. The published beta.2 release is available, but does not imply that these stages are complete.

Queued assignment: implemented on main. Task creation and issue handoff can reserve a registered agent atomically. Creator/operator edits use expected-owner comparison and cannot change running work. Dispatch respects the reserved executor; browser executor setup is implemented.

Executor connectivity: development implementation distinguishes registered keys from recent operator-reported executors, prepares configured identities before their first task and displays last capacity. Browser provisioning and saved final output are implemented; continuous command health checks remain open.

Browser executor setup: implemented exact-repository role/token preparation,
separate configuration and private credential downloads, native launch and a
verified connection state. Operator-owned host installation remains separate.
Next: durable run output, retries/cancellation UX and direct links from a run to
its candidate checks, review and integration outcome.

Saved run output: opt-in final stdout/stderr tails with bounded storage, known-secret
redaction, download and deletion tombstones. Live streaming and exhaustive secret
detection are not claimed.

Task history: task-filtered proposal/run listing and run continuation tokens now
back the browser page. Older task results stay reachable after unrelated repository
activity; explicit load-more and live-refresh retention are locally qualified.
Executor installation is included in the beta.4 candidate archives. The next
workflow gap remains rich review conversations and safe failed-run recovery, not
additional repository-wide test infrastructure.

Retry-budget recovery now follows complete task-scoped history and refuses
truncated legacy history. This prevents unrelated agent work from granting extra
attempts after restart. Included in the beta.4 candidate before publication.

Run result navigation: permanent run pages now list exact-session change requests directly, including state, target stream and continuation pages. Opening a result leads to its diff/check/review workbench; live log streaming and rich review threads remain open.
