# Print an optspec for argparse to handle cmd's options that are independent of any subcommand.
function __fish_ag_global_optspecs
    string join \n C/repo= as= json color= q/quiet v/verbose h/help V/version
end

function __fish_ag_needs_command
    # Figure out if the current invocation already has a command.
    set -l cmd (commandline -opc)
    set -e cmd[1]
    argparse -s (__fish_ag_global_optspecs) -- $cmd 2>/dev/null
    or return
    if set -q argv[1]
        # Also print the command, so this can be used to figure out what it is.
        echo $argv[1]
        return 1
    end
    return 0
end

function __fish_ag_using_subcommand
    set -l cmd (__fish_ag_needs_command)
    test -z "$cmd"
    and return 1
    contains -- $cmd[1] $argv
end

complete -c ag -n "__fish_ag_needs_command" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_needs_command" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_needs_command" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_needs_command" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_needs_command" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_needs_command" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_needs_command" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c ag -n "__fish_ag_needs_command" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_needs_command" -f -a "init" -d 'Create a repository in the current directory'
complete -c ag -n "__fish_ag_needs_command" -f -a "migrate" -d 'Migrate every branch and tag from a full local Git clone or bundle into a new repository'
complete -c ag -n "__fish_ag_needs_command" -f -a "status" -d 'Summarize the workspace, your proposals, leases, task and remote at a glance'
complete -c ag -n "__fish_ag_needs_command" -f -a "snapshot" -d 'Record the working directory as a new snapshot'
complete -c ag -n "__fish_ag_needs_command" -f -a "sync" -d 'Bring the stream\'s latest head into the workspace'
complete -c ag -n "__fish_ag_needs_command" -f -a "log" -d 'Show history'
complete -c ag -n "__fish_ag_needs_command" -f -a "show" -d 'Show one changeset and what it changed'
complete -c ag -n "__fish_ag_needs_command" -f -a "diff" -d 'Show differences between two revisions, or the working directory and its snapshot'
complete -c ag -n "__fish_ag_needs_command" -f -a "cat" -d 'Print a file as of a revision: `ag cat main:src/lib.rs`'
complete -c ag -n "__fish_ag_needs_command" -f -a "blame" -d 'Show which changeset introduced each line of a file'
complete -c ag -n "__fish_ag_needs_command" -f -a "ls" -d 'List a directory as of a revision'
complete -c ag -n "__fish_ag_needs_command" -f -a "checkout" -d 'Write a revision\'s files into a new directory'
complete -c ag -n "__fish_ag_needs_command" -f -a "portability" -d 'List what a revision contains that Windows, macOS or Linux file systems cannot hold'
complete -c ag -n "__fish_ag_needs_command" -f -a "stream" -d 'Manage streams'
complete -c ag -n "__fish_ag_needs_command" -f -a "propose" -d 'Publish a snapshot and open a proposal to integrate it'
complete -c ag -n "__fish_ag_needs_command" -f -a "proposal" -d 'List, review, update or close proposals'
complete -c ag -n "__fish_ag_needs_command" -f -a "integrate" -d 'Integrate a proposal into its stream'
complete -c ag -n "__fish_ag_needs_command" -f -a "preview" -d 'Show what integrating a proposal would do, without doing it'
complete -c ag -n "__fish_ag_needs_command" -f -a "explain" -d 'Say why a proposal does not land yet, in plain words, with the exact commands that fix each reason'
complete -c ag -n "__fish_ag_needs_command" -f -a "op" -d 'Inspect or resume operations'
complete -c ag -n "__fish_ag_needs_command" -f -a "recover" -d 'Resume interrupted operations and expire stale ones'
complete -c ag -n "__fish_ag_needs_command" -f -a "queue" -d 'The ordered integration queue'
complete -c ag -n "__fish_ag_needs_command" -f -a "policy" -d 'Show or change stream policy'
complete -c ag -n "__fish_ag_needs_command" -f -a "validate" -d 'Run checks and issue signed attestations'
complete -c ag -n "__fish_ag_needs_command" -f -a "agent" -d 'Agent identities and runs'
complete -c ag -n "__fish_ag_needs_command" -f -a "task" -d 'Tasks and their dependencies'
complete -c ag -n "__fish_ag_needs_command" -f -a "lease" -d 'Soft and exclusive leases with fencing tokens'
complete -c ag -n "__fish_ag_needs_command" -f -a "provenance" -d 'Signed provenance of agent-made changes'
complete -c ag -n "__fish_ag_needs_command" -f -a "predict" -d 'Predict overlapping work'
complete -c ag -n "__fish_ag_needs_command" -f -a "workspace" -d 'Create, list and remove workspaces (isolated lines of work in their own directories)'
complete -c ag -n "__fish_ag_needs_command" -f -a "trace" -d 'Show the audit trail of an object, proposal, run or stream'
complete -c ag -n "__fish_ag_needs_command" -f -a "audit" -d 'Verify the tamper-evident audit chain'
complete -c ag -n "__fish_ag_needs_command" -f -a "verify" -d 'Check the integrity of the whole repository'
complete -c ag -n "__fish_ag_needs_command" -f -a "gc" -d 'Delete unreachable objects (safe: fails closed, honours a grace period)'
complete -c ag -n "__fish_ag_needs_command" -f -a "reindex" -d 'Rebuild derived indexes from the object store'
complete -c ag -n "__fish_ag_needs_command" -f -a "backup" -d 'Write a verified, consistent copy of the repository to a new directory'
complete -c ag -n "__fish_ag_needs_command" -f -a "git" -d 'Import from or export to Git (needs the `git` command)'
complete -c ag -n "__fish_ag_needs_command" -f -a "clone" -d 'Clone a repository from an `ag-server`'
complete -c ag -n "__fish_ag_needs_command" -f -a "fetch" -d 'Download new history from the server'
complete -c ag -n "__fish_ag_needs_command" -f -a "pull" -d 'Fetch, then fast-forward the local stream and bring the workspace up to date'
complete -c ag -n "__fish_ag_needs_command" -f -a "push" -d 'Upload your snapshot and open a proposal on the server'
complete -c ag -n "__fish_ag_needs_command" -f -a "remote" -d 'Manage remotes and work with proposals, events and policy on a server'
complete -c ag -n "__fish_ag_needs_command" -f -a "login" -d 'Save a server credential once; `clone`, `fetch`, `push` and `remote` then need no token plumbing'
complete -c ag -n "__fish_ag_needs_command" -f -a "auth" -d 'Inspect saved server connections without printing credentials'
complete -c ag -n "__fish_ag_needs_command" -f -a "logout" -d 'Forget a saved server credential'
complete -c ag -n "__fish_ag_needs_command" -f -a "pin" -d 'Pin a revision so garbage collection keeps it'
complete -c ag -n "__fish_ag_needs_command" -f -a "unpin" -d 'Remove a pin'
complete -c ag -n "__fish_ag_needs_command" -f -a "pins" -d 'List pins'
complete -c ag -n "__fish_ag_needs_command" -f -a "doctor" -d 'Check the installation, the repository, your identity and every remote\'s credential'
complete -c ag -n "__fish_ag_needs_command" -f -a "config" -d 'Show, get, set and locate configuration (the user file; the repository\'s settings are shown read-only)'
complete -c ag -n "__fish_ag_needs_command" -f -a "completions" -d 'Print a shell completion script: bash, zsh, fish, powershell or elvish'
complete -c ag -n "__fish_ag_needs_command" -f -a "man" -d 'Print the man page, or write one page per command with --dir'
complete -c ag -n "__fish_ag_needs_command" -f -a "version" -d 'Print the version, the protocol and the JSON schema version'
complete -c ag -n "__fish_ag_needs_command" -f -a "help" -d 'Show help for a command, or a topic: workflow, policy, leases, identities, revisions, exit-codes, json, config'
complete -c ag -n "__fish_ag_using_subcommand init" -l namespace -d 'Repository namespace, e.g. `acme/webapp` (default: `local/<directory name>`)' -r
complete -c ag -n "__fish_ag_using_subcommand init" -l stream -d 'Name of the first stream' -r
complete -c ag -n "__fish_ag_using_subcommand init" -l storage -d 'JSON file with the `storage` section (PostgreSQL metadata and/or S3 objects); see docs/operations/postgres-and-s3.md. Default: a SQLite file and local objects under `.ag/`' -r -F
complete -c ag -n "__fish_ag_using_subcommand init" -l preset -d 'Store a ready-made policy for the first stream: solo, team, agents or strict (`ag policy preset list`)' -r
complete -c ag -n "__fish_ag_using_subcommand init" -l admin -d 'Identity pattern allowed to change the policy (repeatable). Default: you' -r
complete -c ag -n "__fish_ag_using_subcommand init" -l issuer -d 'Trust this CI issuer to sign the `tests` check: IDENTITY=PUBLIC_KEY, e.g. service:ci=<64 hex digits>' -r
complete -c ag -n "__fish_ag_using_subcommand init" -l ci-key -d 'Trust the signing key NAME of this repository (created if missing; `ag validate keygen NAME` makes the same key): the issuer becomes service:NAME' -r
complete -c ag -n "__fish_ag_using_subcommand init" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand init" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand init" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand init" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand init" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand init" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand init" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand init" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand migrate" -l namespace -d 'Destination namespace, for example acme/webapp' -r
complete -c ag -n "__fish_ag_using_subcommand migrate" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand migrate" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand migrate" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand migrate" -l dry-run -d 'Verify the migration and report limitations without publishing a repository'
complete -c ag -n "__fish_ag_using_subcommand migrate" -l allow-lossy -d 'Accept reported omissions, unresolved LFS pointers and unsupported refs'
complete -c ag -n "__fish_ag_using_subcommand migrate" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand migrate" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand migrate" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand migrate" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand migrate" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand status" -s w -l workspace -d 'Workspace name' -r
complete -c ag -n "__fish_ag_using_subcommand status" -l dir -d 'Working directory of the workspace (required for non-default workspaces)' -r -F
complete -c ag -n "__fish_ag_using_subcommand status" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand status" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand status" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand status" -l remote -d 'Also ask each remote\'s server for its current head (needs the network and a credential)'
complete -c ag -n "__fish_ag_using_subcommand status" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand status" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand status" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand status" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand status" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s m -l message -d 'Snapshot message' -r
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s w -l workspace -d 'Workspace name' -r
complete -c ag -n "__fish_ag_using_subcommand snapshot" -l dir -d 'Working directory of the workspace' -r -F
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand snapshot" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand snapshot" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand snapshot" -l force -d 'Snapshot even if conflict markers remain'
complete -c ag -n "__fish_ag_using_subcommand snapshot" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand snapshot" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand sync" -s w -l workspace -d 'Workspace name' -r
complete -c ag -n "__fish_ag_using_subcommand sync" -l dir -d 'Working directory of the workspace' -r -F
complete -c ag -n "__fish_ag_using_subcommand sync" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand sync" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand sync" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand sync" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand sync" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand sync" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand sync" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand sync" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand log" -s n -l limit -d 'Maximum number of entries' -r
complete -c ag -n "__fish_ag_using_subcommand log" -l path -d 'Only changesets that touched this path' -r
complete -c ag -n "__fish_ag_using_subcommand log" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand log" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand log" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand log" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand log" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand log" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand log" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand log" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand show" -l copies -d 'Also report files created as copies of another file (`C` lines): identical content, or at least 50% of the bytes in common with another file (co-modified and neighbouring files are tried first)'
complete -c ag -n "__fish_ag_using_subcommand show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand diff" -s U -l context -d 'Lines of context' -r
complete -c ag -n "__fish_ag_using_subcommand diff" -s w -l workspace -d 'Workspace (for the no-argument form)' -r
complete -c ag -n "__fish_ag_using_subcommand diff" -l dir -d 'Workspace directory (for the no-argument form)' -r -F
complete -c ag -n "__fish_ag_using_subcommand diff" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand diff" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand diff" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand diff" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand diff" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand diff" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand diff" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand diff" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand cat" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand cat" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand cat" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand cat" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand cat" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand cat" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand cat" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand cat" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand blame" -s L -l lines -d 'Only these lines, `START,END` (1-based, inclusive)' -r
complete -c ag -n "__fish_ag_using_subcommand blame" -l max-commits -d 'Visit at most this many changesets; older lines are marked as a boundary' -r
complete -c ag -n "__fish_ag_using_subcommand blame" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand blame" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand blame" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand blame" -l copies -d 'Follow files that were created as copies of another file: lines a copy took over are blamed on whoever wrote them in the source (identical content, or at least 50% of the bytes in common with a neighbouring or co-modified file)'
complete -c ag -n "__fish_ag_using_subcommand blame" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand blame" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand blame" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand blame" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand blame" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand ls" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand ls" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand ls" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand ls" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand ls" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand ls" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand ls" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand ls" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand checkout" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand checkout" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand checkout" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand checkout" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand checkout" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand checkout" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand checkout" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand checkout" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand portability" -l target -d 'File system to check against: `windows`, `macos` or `linux` (default: this machine)' -r
complete -c ag -n "__fish_ag_using_subcommand portability" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand portability" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand portability" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand portability" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand portability" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand portability" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand portability" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand portability" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -f -a "list" -d 'List streams and their heads'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -f -a "create" -d 'Create a stream, optionally at an existing published revision'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -f -a "show" -d 'Show one stream'
complete -c ag -n "__fish_ag_using_subcommand stream; and not __fish_seen_subcommand_from list create show reflog" -f -a "reflog" -d 'Show the append-only history of head movements'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -l from -d 'Start the stream at this published revision' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from create" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -s n -l limit -d 'Maximum entries' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand stream; and __fish_seen_subcommand_from reflog" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand propose" -s t -l title -d 'Proposal title' -r
complete -c ag -n "__fish_ag_using_subcommand propose" -s d -l description -d 'Longer description' -r
complete -c ag -n "__fish_ag_using_subcommand propose" -l stream -d 'Target stream (default: the repository\'s default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand propose" -l task -d 'Task this proposal belongs to' -r
complete -c ag -n "__fish_ag_using_subcommand propose" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand propose" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand propose" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand propose" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand propose" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand propose" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand propose" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand propose" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -f -a "list" -d 'List proposals, newest first'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -f -a "show" -d 'Show a proposal with its reviews'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -f -a "reviewers" -d 'Inspect, add or remove requested reviewers (no approval or access change)'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -f -a "review" -d 'Review the proposal\'s current head'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -f -a "update" -d 'Point the proposal at a new head (earlier approvals stop counting)'
complete -c ag -n "__fish_ag_using_subcommand proposal; and not __fish_seen_subcommand_from list show reviewers review update close" -f -a "close" -d 'Close without integrating'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -l state -d 'Filter by state: open, integrated, closed' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -l requested-reviewer -d 'Only proposals that request this reviewer, matched before the limit' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -s n -l limit -d 'Show at most this many (0 = all)' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l add -d 'Add a requested reviewer; repeat for several identities' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l remove -d 'Remove a requested reviewer; repeat for several identities' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l expected -d 'Require this complete inspected set; repeat for each identity' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l expect-empty -d 'Require that no reviewers have been requested yet'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from reviewers" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -s m -l message -d 'Review text' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -l approve -d 'Approve'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -l request-changes -d 'Request changes'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -l comment -d 'Comment only'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from review" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -l expected-head -d 'Refuse the update if the proposal no longer has this inspected head' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from update" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand proposal; and __fish_seen_subcommand_from close" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand integrate" -l key -l idempotency-key -d 'Idempotency key (default: derived from the proposal; repeating resumes/replays). Not the issuer key of `ag validate run --key`; `--idempotency-key` is the unambiguous spelling' -r
complete -c ag -n "__fish_ag_using_subcommand integrate" -l fencing-token -d 'Fencing token(s) of exclusive leases the author holds' -r
complete -c ag -n "__fish_ag_using_subcommand integrate" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand integrate" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand integrate" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand integrate" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand integrate" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand integrate" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand integrate" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand integrate" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand preview" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand preview" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand preview" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand preview" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand preview" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand preview" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand preview" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand preview" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand explain" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand explain" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand explain" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand explain" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand explain" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand explain" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand explain" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand explain" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -f -a "show" -d 'Show an operation\'s recorded state and result. Use this instead of blindly retrying when a response was lost'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -f -a "resume" -d 'Resume an operation (for example one that was blocked)'
complete -c ag -n "__fish_ag_using_subcommand op; and not __fish_seen_subcommand_from show resume list" -f -a "list" -d 'List recent operations'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -l fencing-token -d 'Fencing token(s) to present' -r
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from resume" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -s n -l limit -d 'Maximum entries' -r
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand op; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand recover" -l max-age -d 'Operations untouched for longer than this are aborted' -r
complete -c ag -n "__fish_ag_using_subcommand recover" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand recover" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand recover" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand recover" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand recover" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand recover" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand recover" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand recover" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -f -a "add" -d 'Add a proposal to its stream\'s queue'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -f -a "list" -d 'Show the queue'
complete -c ag -n "__fish_ag_using_subcommand queue; and not __fish_seen_subcommand_from add list run" -f -a "run" -d 'Integrate queued proposals in order'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from add" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -l stream -d 'The stream, as an option (the same as the positional STREAM)' -r
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -l stream -d 'The stream, as an option (the same as the positional STREAM)' -r
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand queue; and __fish_seen_subcommand_from run" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -f -a "show" -d 'Show the policy in force'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -f -a "set" -d 'Replace the policy with the contents of a JSON file (needs an admin)'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -f -a "history" -d 'List policy versions'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -f -a "example" -d 'Print an example policy document'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -f -a "preset" -d 'Ready-made policies: solo, team, agents, strict'
complete -c ag -n "__fish_ag_using_subcommand policy; and not __fish_seen_subcommand_from show set history example preset explain" -f -a "explain" -d 'Say in plain language what a policy allows, requires and refuses'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -l stream -d 'The stream, as an option (the same as the positional STREAM)' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -l stream -d 'Stream (default: the default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from set" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -l stream -d 'The stream, as an option (the same as the positional STREAM)' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from history" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from example" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -f -a "list" -d 'List the presets and who each is for'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -f -a "show" -d 'Print the policy document a preset makes, and what it means'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from preset" -f -a "apply" -d 'Store a preset as the stream\'s next policy version (needs an admin)'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -l stream -d 'Stream whose policy `current` means (default: the default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand policy; and __fish_seen_subcommand_from explain" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -f -a "keygen" -d 'Generate a signing key for an issuer and print its public key'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -f -a "run" -d 'Check out the exact candidate tree of a proposal, run a command on it and record a signed attestation of the result'
complete -c ag -n "__fish_ag_using_subcommand validate; and not __fish_seen_subcommand_from keygen run list" -f -a "list" -d 'List attestations recorded for a tree'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from keygen" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l check -d 'Check name, as listed in the policy\'s `required_validations`' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l key -d 'Name of the issuer key created with `ag validate keygen`' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l issuer -d 'Issuer identity (default: `service:<key name>`)' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l env -d 'Environment digest to record (default: derived from the command and platform)' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -s h -l help -d 'Print help (see more with \'--help\')'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from run" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand validate; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "register" -d 'Register an agent and create its signing key'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "list" -d 'List agents'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "start" -d 'Start a run. Prints the identity the agent must use (`agent:name#run_…`)'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "finish" -d 'Finish a run and release its leases'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "heartbeat" -d 'Heartbeat a running run'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "runs" -d 'List runs, newest first'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "reap" -d 'Abandon runs with no recent heartbeat and release their leases'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "disable" -d 'Stop an agent from starting new runs'
complete -c ag -n "__fish_ag_using_subcommand agent; and not __fish_seen_subcommand_from register list start finish heartbeat runs reap disable enable" -f -a "enable" -d 'Allow a disabled agent to run again'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -l role -d '`agent` or `reviewer`' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from register" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -l remote -d 'Read a server\'s agents instead of the local repository (without a NAME: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -l task -d 'Task to work on (claims it; its dependencies must be done)' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from start" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -l status -d '`completed` or `failed`' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -s m -l message -d 'Summary' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from finish" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from heartbeat" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -s n -l limit -d 'Show at most this many (0 = all)' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from runs" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -l timeout -d 'Heartbeat timeout (default 10m here; with `--remote`, the server\'s `agents.run_stale_after_secs`)' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -l remote -d 'Reap the runs of a server instead of this repository (needs `agent.operate` there; without a NAME: origin). The server also revokes the credentials bound to the reaped runs' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -l dry-run -d 'Say what would be abandoned without changing anything'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from reap" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -l remote -d 'Change the server\'s agent instead (requires agent.operate; without a NAME: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from disable" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -l remote -d 'Change the server\'s agent instead (requires agent.operate; without a NAME: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand agent; and __fish_seen_subcommand_from enable" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "create" -d 'Create a task'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "edit" -d 'Refine instructions before a task\'s first run; concurrent edits are refused'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "attempts" -d 'Inspect or change a task\'s durable execution-attempt limit'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "list" -d 'List tasks'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "depend" -d 'Declare that TASK cannot start until ON is done'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "ready" -d 'Open tasks whose dependencies are all done'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "done" -d 'Mark a task done'
complete -c ag -n "__fish_ag_using_subcommand task; and not __fish_seen_subcommand_from create edit attempts list depend ready done cancel" -f -a "cancel" -d 'Cancel a task'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -s d -l description -d 'Description' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l scope -d 'Path glob the task is expected to touch (repeatable)' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l source-task -d 'Original task for an independent follow-up' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l source-run -d 'Failed or abandoned run of --source-task' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l max-attempts -d 'Total runs permitted (0 leaves the server limit unset)' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from create" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l title -d 'New title; omitted keeps the current title' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l description -d 'New description; an empty string clears it' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l scope -d 'Replace path scope (repeatable)' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l remote -d 'Edit on a configured server instead of locally (default name: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l clear-scope -d 'Remove the expected file-path scope (repository policy still applies)'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from edit" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -l max -d 'New maximum total runs; zero leaves the server limit unset' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -l expected -d 'Limit you inspected; refuses a concurrent policy change' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -l remote -d 'Use a configured server instead of the local repository' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from attempts" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -l state -d 'Filter by state: open, in_progress, done, cancelled' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from depend" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from ready" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from done" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand task; and __fish_seen_subcommand_from cancel" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -f -a "acquire" -d 'Claim a scope (path glob). Soft by default; `--exclusive` is enforced at integration'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -f -a "release" -d 'Release a lease early'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -f -a "extend" -d 'Extend a lease; the fencing token stays the same'
complete -c ag -n "__fish_ag_using_subcommand lease; and not __fish_seen_subcommand_from acquire release extend list" -f -a "list" -d 'List active leases'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -l ttl -d 'How long the lease lasts' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -l task -d 'Task the lease belongs to' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -l exclusive -d 'Enforce with a fencing token at integration time'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from acquire" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from release" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -l ttl -d 'New duration from now' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from extend" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -s n -l limit -d 'With --all: show at most this many (0 = all)' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -l all -d 'Include released and expired leases (the newest ones, see -n)'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand lease; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -f -a "attach" -d 'Sign and attach a provenance record to a changeset'
complete -c ag -n "__fish_ag_using_subcommand provenance; and not __fish_seen_subcommand_from attach show" -f -a "show" -d 'Show and verify the provenance of a changeset'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l agent -d 'Agent name (its key signs the record)' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l run -d 'Run id' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l task -d 'Task id' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l model -d 'Model identifier' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l prompt-file -d 'File holding the prompt; only its SHA-256 is stored' -r -F
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l rationale -d 'Why the change was made' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l check -d 'A check the agent ran itself (repeatable)' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l input -d 'A source consulted (repeatable). Stored as untrusted text' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from attach" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand provenance; and __fish_seen_subcommand_from show" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand predict" -l stream -d 'The stream, as an option (the same as the positional STREAM)' -r
complete -c ag -n "__fish_ag_using_subcommand predict" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand predict" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand predict" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand predict" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand predict" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand predict" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand predict" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand predict" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -f -a "create" -d 'Create a workspace starting at the stream head and check it out into DIR'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -f -a "list" -d 'List workspaces'
complete -c ag -n "__fish_ag_using_subcommand workspace; and not __fish_seen_subcommand_from create list remove" -f -a "remove" -d 'Forget a workspace (its directory is left alone)'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -l dir -d 'Directory to check out into (created if missing; must be empty)' -r -F
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -l stream -d 'Stream to track (default: the default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from create" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand workspace; and __fish_seen_subcommand_from remove" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand trace" -s n -l limit -d 'Maximum entries' -r
complete -c ag -n "__fish_ag_using_subcommand trace" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand trace" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand trace" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand trace" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand trace" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand trace" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand trace" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand trace" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand audit" -s n -l limit -d 'Number of entries for `--list`' -r
complete -c ag -n "__fish_ag_using_subcommand audit" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand audit" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand audit" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand audit" -l list -d 'List recent entries instead of verifying the chain'
complete -c ag -n "__fish_ag_using_subcommand audit" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand audit" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand audit" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand audit" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand audit" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand verify" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand verify" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand verify" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand verify" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand verify" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand verify" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand verify" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand verify" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand gc" -l grace -d 'Keep unreachable objects younger than this' -r
complete -c ag -n "__fish_ag_using_subcommand gc" -l reflog-retention -d 'Keep heads that appeared in a reflog within this window' -r
complete -c ag -n "__fish_ag_using_subcommand gc" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand gc" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand gc" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand gc" -l dry-run -d 'Report only; delete nothing'
complete -c ag -n "__fish_ag_using_subcommand gc" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand gc" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand gc" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand gc" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand gc" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand reindex" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand reindex" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand reindex" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand reindex" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand reindex" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand reindex" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand reindex" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand reindex" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand backup" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand backup" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand backup" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand backup" -l include-keys -d 'Also copy private keys (agent and issuer identities). Off by default'
complete -c ag -n "__fish_ag_using_subcommand backup" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand backup" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand backup" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand backup" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand backup" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -f -a "import" -d 'Import a Git repository\'s history into a new stream'
complete -c ag -n "__fish_ag_using_subcommand git; and not __fish_seen_subcommand_from import export" -f -a "export" -d 'Export a stream\'s history to a Git repository (created if missing)'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -l rev -d 'Revision to import' -r
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -l stream -d 'Stream to create for the imported history' -r
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from import" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -l branch -d 'Branch name in Git' -r
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand git; and __fish_seen_subcommand_from export" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand clone" -l stream -d 'Stream to clone (default: the repository\'s default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand clone" -l name -d 'Name for the remote' -r
complete -c ag -n "__fish_ag_using_subcommand clone" -l token-env -d 'Environment variable holding the credential (default: AG_TOKEN)' -r
complete -c ag -n "__fish_ag_using_subcommand clone" -l ca-file -d 'Extra CA certificate (PEM) to trust' -r -F
complete -c ag -n "__fish_ag_using_subcommand clone" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand clone" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand clone" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand clone" -l token-stdin -d 'Read the credential from stdin and store it in `.ag/credentials/<name>` (mode 0600)'
complete -c ag -n "__fish_ag_using_subcommand clone" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand clone" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand clone" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand clone" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand clone" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand fetch" -l remote -d 'Remote name (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand fetch" -l stream -d 'Stream (default: the repository\'s default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand fetch" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand fetch" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand fetch" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand fetch" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand fetch" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand fetch" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand fetch" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand fetch" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand pull" -l remote -d 'Remote name (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand pull" -l stream -d 'Stream (default: the repository\'s default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand pull" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand pull" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand pull" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand pull" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand pull" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand pull" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand pull" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand pull" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand push" -l remote -d 'Remote name (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand push" -s t -l title -d 'Proposal title' -r
complete -c ag -n "__fish_ag_using_subcommand push" -s d -l description -d 'Longer description' -r
complete -c ag -n "__fish_ag_using_subcommand push" -l stream -d 'Target stream (default: the repository\'s default stream)' -r
complete -c ag -n "__fish_ag_using_subcommand push" -l task -d 'Task this proposal belongs to' -r
complete -c ag -n "__fish_ag_using_subcommand push" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand push" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand push" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand push" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand push" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand push" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand push" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand push" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "add" -d 'Add a remote'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "list" -d 'List remotes'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "remove" -d 'Forget a remote'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "whoami" -d 'Show who the server thinks you are'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "proposals" -d 'List proposals on the server'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "proposal" -d 'Show a proposal with its reviews'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "prepare" -d 'What would integrating do now, and what is missing?'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "explain" -d 'Explain why a server proposal cannot land, with the next commands. Changes nothing'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "reviewers" -d 'Inspect, add or remove requested reviewers (no approval or access change)'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "review-draft" -d 'Save or restore your private account review notes without submitting them'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "review" -d 'Review a proposal\'s current head'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "discussion" -d 'Read and participate in revision-bound line discussions'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "integrate" -d 'Integrate a proposal into its stream'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "close" -d 'Close a proposal without integrating it'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "streams" -d 'List streams'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "events" -d 'Show the events of the repository (replay), or follow them live'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "policy" -d 'Show or set a stream\'s policy'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "log" -d 'Show history of a revision on the server'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "blame" -d 'Show which changeset introduced each line of a file on the server: `ag remote blame main:src/lib.rs`'
complete -c ag -n "__fish_ag_using_subcommand remote; and not __fish_seen_subcommand_from add list remove whoami proposals proposal prepare explain reviewers review-draft review discussion integrate close streams events policy log blame cat" -f -a "cat" -d 'Print a file as of a revision on the server: `ag remote cat main:src/lib.rs`'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l token-env -d 'Environment variable holding the credential' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l ca-file -d 'Extra CA certificate (PEM) to trust' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l token-stdin -d 'Read the credential from stdin and store it in `.ag/credentials/<name>` (mode 0600)'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from add" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from remove" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from whoami" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -l state -d 'Filter: open, integrated, closed' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -l requested-reviewer -d 'Only proposals that request this reviewer, matched before paging' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposals" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from proposal" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -l validate -d 'Also queue the validation jobs the policy knows how to run'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from prepare" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from explain" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l add -d 'Add a requested reviewer; repeat for several identities' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l remove -d 'Remove a requested reviewer; repeat for several identities' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l expected -d 'Require this complete inspected set; repeat for each identity' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l expect-empty -d 'Require that no reviewers have been requested yet'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from reviewers" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -f -a "get" -d 'Read your private draft and its version, including a cleared version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -f -a "save" -d 'Save private notes; does not submit a review or advance the inspected head'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review-draft" -f -a "clear" -d 'Clear only the private draft version you inspected'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -s m -l message -d 'Review text' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l approve -d 'Approve'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l request-changes -d 'Request changes'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l comment -d 'Comment only'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from review" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -f -a "list" -d 'List line discussions, including resolved and earlier-head threads'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -f -a "replies" -d 'Read replies to a line discussion'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -f -a "start" -d 'Start a thread on the exact revision and line you inspected'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -f -a "reply" -d 'Reply to an open line discussion'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from discussion" -f -a "resolve" -d 'Resolve or reopen a thread only at the version you read'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -l fencing-token -d 'Fencing tokens of exclusive leases the author holds' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from integrate" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from close" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from streams" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l after -d 'Replay events after this sequence number' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l topics -d 'Only these topics (prefix match), e.g. `stream,proposal`' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l follow -d 'Keep following new events'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from events" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l set -d 'Replace the policy with the JSON document in this file' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l preset -d 'Replace the policy with a ready-made one: solo, team, agents or strict (`ag policy preset list`)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l admin -d 'Identity pattern allowed to change the policy (repeatable). Default: you' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l issuer -d 'Trust this CI issuer to sign the `tests` check: IDENTITY=PUBLIC_KEY, e.g. service:ci=<64 hex digits>' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l ci-key -d 'Trust the signing key NAME of this repository (created if missing; `ag validate keygen NAME` makes the same key): the issuer becomes service:NAME' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l explain -d 'Also say in plain language what the policy allows, requires and refuses'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from policy" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -s n -l limit -d 'Maximum entries' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from log" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -s L -l lines -d 'Only these lines, `START,END`' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -l max-commits -d 'Look at most this many changesets back (default: the server\'s limit; the output says when it stopped)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from blame" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -l remote -d 'Remote to use (default: origin)' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand remote; and __fish_seen_subcommand_from cat" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand login" -l token-file -d 'Read the credential from a file (`ag-server dev` writes one and prints the command)' -r -F
complete -c ag -n "__fish_ag_using_subcommand login" -l ca-file -d 'Extra CA certificate (PEM) to trust when checking the credential' -r -F
complete -c ag -n "__fish_ag_using_subcommand login" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand login" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand login" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand login" -l token-stdin -d 'Read the credential from standard input (`echo "$TOKEN" | ag login URL --token-stdin`)'
complete -c ag -n "__fish_ag_using_subcommand login" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand login" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand login" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand login" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand login" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand auth; and not __fish_seen_subcommand_from status" -f -a "status" -d 'List saved connections; --check asks each server whether its saved credential still works'
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -l timeout -d 'Timeout for each online check, in seconds' -r
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -l ca-file -d 'Extra CA certificate (PEM) for online checks' -r -F
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -l check -d 'Verify saved credentials online. Without this, status is local and works offline'
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand auth; and __fish_seen_subcommand_from status" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand logout" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand logout" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand logout" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand logout" -l all -d 'Forget every saved login'
complete -c ag -n "__fish_ag_using_subcommand logout" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand logout" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand logout" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand logout" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand logout" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand pin" -l reason -d 'Why' -r
complete -c ag -n "__fish_ag_using_subcommand pin" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand pin" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand pin" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand pin" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand pin" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand pin" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand pin" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand pin" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand unpin" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand unpin" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand unpin" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand unpin" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand unpin" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand unpin" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand unpin" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand unpin" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand pins" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand pins" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand pins" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand pins" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand pins" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand pins" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand pins" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand pins" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand doctor" -l timeout -d 'How long to wait for each server before giving up' -r
complete -c ag -n "__fish_ag_using_subcommand doctor" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand doctor" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand doctor" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand doctor" -l offline -d 'Do not contact any server: check that credentials exist, not that they work'
complete -c ag -n "__fish_ag_using_subcommand doctor" -l strict -d 'Exit with 1 when a check warns, not only when one fails'
complete -c ag -n "__fish_ag_using_subcommand doctor" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand doctor" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand doctor" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand doctor" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand doctor" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -f -a "list" -d 'Show every setting with its effective value and where it comes from'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -f -a "get" -d 'Print one setting\'s effective value (see `list` for the keys)'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -f -a "set" -d 'Store a user-level setting: `identity` or `color`'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -f -a "unset" -d 'Remove a stored user-level setting'
complete -c ag -n "__fish_ag_using_subcommand config; and not __fish_seen_subcommand_from list get set unset path" -f -a "path" -d 'Print the locations of the configuration files'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from list" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from get" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from set" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from unset" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand config; and __fish_seen_subcommand_from path" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand completions" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand completions" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand completions" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand completions" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand completions" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand completions" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand completions" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand completions" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand man" -l dir -d 'Write `ag.1` and one page per subcommand (`ag-stream-list.1`, …) into this directory instead of printing the top-level page' -r -F
complete -c ag -n "__fish_ag_using_subcommand man" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand man" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand man" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand man" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand man" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand man" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand man" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand man" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand version" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand version" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand version" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand version" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand version" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand version" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand version" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand version" -s V -l version -d 'Print version'
complete -c ag -n "__fish_ag_using_subcommand help" -s C -l repo -d 'Operate on the repository at this path (default: discovered from the current directory)' -r -F
complete -c ag -n "__fish_ag_using_subcommand help" -l as -d 'Act as this identity (`user:alice`, `agent:bot#run_…`, `service:ci`). Also: AG_IDENTITY' -r
complete -c ag -n "__fish_ag_using_subcommand help" -l color -d 'Colour the output: auto (a terminal, unless NO_COLOR is set), always or never' -r -f -a "auto\t'Colour when the stream is a terminal and neither `NO_COLOR` nor `TERM=dumb` says otherwise'
always\t'Always colour'
never\t'Never colour'"
complete -c ag -n "__fish_ag_using_subcommand help" -l json -d 'Machine-readable JSON output'
complete -c ag -n "__fish_ag_using_subcommand help" -s q -l quiet -d 'Print only what was asked for: no confirmations or hints. Errors and refusals are still shown'
complete -c ag -n "__fish_ag_using_subcommand help" -s v -l verbose -d 'Explain what is happening on standard error: repository, identity, remote, timing'
complete -c ag -n "__fish_ag_using_subcommand help" -s h -l help -d 'Print help'
complete -c ag -n "__fish_ag_using_subcommand help" -s V -l version -d 'Print version'
