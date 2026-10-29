Workflow: from an edit to a landed change

A change goes through four independent checks. None implies another:

    Mergeable   is there one unambiguous result?        ag preview PROPOSAL
    Validated   did trusted checks pass on THAT tree?   ag validate run PROPOSAL ...
    Approved    did enough eligible people approve?     ag proposal review PROPOSAL --approve
    Integrated  was the stream updated atomically?      ag integrate PROPOSAL

The everyday loop

    ag init --namespace me/project     once: creates .ag/ and the stream "main"
    ag snapshot -m "message"           record the working directory (private until proposed)
    ag propose -t "title"              publish the snapshot and open a proposal
    ag integrate PROPOSAL              move the stream; policy decides whether it may

Any unique prefix (6 or more characters) of a proposal id works: `ag integrate prop_8f3a1c`.

When integrate says "Blocked" (exit code 3)

The stream was not changed. The message lists what is missing; provide it and run the SAME command again, it
resumes the same operation:

    missing validation   ag validate run PROPOSAL --check NAME --key ISSUER -- COMMAND
    missing approval     ag proposal review PROPOSAL --approve          (by someone eligible)
    missing provenance   ag provenance attach @WORKSPACE --agent NAME
    lease token          ag integrate PROPOSAL --fencing-token N

When it says "Conflict" or "Denied" (exit code 4)

The stream was not changed and waiting will not help:

    conflict   ag sync, resolve the marked files, ag snapshot, ag proposal update PROPOSAL
    denied     the policy forbids it for this actor or change; see `ag help policy`

Working in parallel

    ag workspace create NAME --dir ../NAME     an isolated directory that starts at the stream head
    ag snapshot -m MSG --workspace NAME
    ag propose @NAME -t TITLE
    ag sync --workspace NAME                   bring the stream's new head into the workspace

Where am I? `ag status` shows the workspace, your proposals, your leases, your task and the remote.
Is everything healthy? `ag doctor`. What exactly happened? `ag trace`, `ag op show OPERATION`.

More: docs/getting-started.md and docs/tutorials/first-agent-safe-repo.md

Requesting a review

    ag proposal reviewers PROPOSAL                         inspect the requested people
    ag proposal reviewers PROPOSAL --add user:bob           request Bob's review
    ag proposal reviewers PROPOSAL --remove user:bob        withdraw that request
    ag proposal list --requested-reviewer user:bob          find Bob's requested work

The same commands work against a configured server as `ag remote reviewers PROPOSAL ...`
and `ag remote proposals --requested-reviewer user:bob`.
Only the author of an open proposal can change its requested reviewers. A request is routing:
it grants no repository access, records no approval, and adds no integration requirement.
A new proposal revision keeps the requested people, but previous approvals remain tied to their old head.

Edits compare the list that the command just read. If another writer changes it before the save, the command
fails instead of overwriting that change. Scripts can require their own inspected list with repeatable
`--expected IDENTITY`, or `--expect-empty` for an empty list. For example:

    ag proposal reviewers PROPOSAL --add user:carol --expected user:bob

An omitted edit flag only reads the list. Repeating an already-applied add or remove is safe.
