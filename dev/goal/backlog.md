# Ordered platform backlog

Reference: [objective](../../doc/goal.md), [full matrix](coverage.csv), [evidence](evidence.md).
All original 142 matrix rows remain in scope; the added tree.search API row brings the inventory to 143 rows. This ordering is a sequence of verifiable slices, not a reduced definition of readiness.

| Stage | Outcome and acceptance | Current state |
| --- | --- | --- |
| P0 editor ownership | A browser-created edit submits once, retains its original session across retries, preserves the draft after cancellation, and cannot redirect a different session. Prove both a clean-server UI path and negative transport cases. | Merged in PR74; targeted real-browser checks and hosted gates passed. |
| P0 confidential-draft boundary | Define who can read draft metadata and referenced objects before adding account-backed autosave. Test owner, other reader, unrelated project and admin; document compatibility. | Open. Workspace read/list is currently repository-readable. |
| P1 code search | Find literal content at a revision, continue a bounded scan and open the pinned matching line. | Implemented and locally qualified through HTTP, gRPC and two real-browser profiles; hosted candidate checks pending. Exclusions are explicit; indexed/global/regex search remains open. |
| P1 multi-file editing | Edit, add, rename and delete several files in one browser change request; preview the exact combined diff, preserve base/modes and recover the batch. No CLI preparation counts. | Basic editor merged in PR75 and verified in two browser profiles; broader role, large-batch and device qualification remains. |
| P1 review conversations | Durable line threads, resolution, suggestions and requested reviewers tied to an inspected head; stale approval remains invalid. | Open; single review comments are not a substitute. |
| P1 task navigation | Permanent task pages, prerequisite/dependant links and confirmed task completion/cancellation over real APIs. | Implemented and locally verified in Chromium and phone viewport; server-backed issue tracker and full agent execution remain separate work. |
| P1 identity and tracker | Organization/member/invitation lifecycle and user issues connected to orchestrator tasks, with role checks and history. | Open; see individual matrix rows. |
| P2 execution | Configure a real executor from UI, run a DAG, retain logs/artifacts, cancel and safely retry with bounded resources. | Open; scripted stand-ins do not qualify this stage. |
| P2 administration and portability | Browser setup/diagnostics, verified restore, hosted-data migration, release/registry/webhook lifecycle. | Open; assess each subsystem separately. |
| External qualification | Real MCP host, clean macOS, physical device, human screen reader, independent pilots, comparable pinned-competitor benchmark. | Open issues #50, #51, #52, #53 and #55; unavailable equipment/participants are not replaced by simulation. |

Baseline for this resumed work: main `549555c82219c6465e59b4d16a3d5a9d3bea47f9`, 2026-10-28. The published beta.2 release is available, but does not imply that these stages are complete.
