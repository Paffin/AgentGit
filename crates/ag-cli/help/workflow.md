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
