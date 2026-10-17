Exit codes: what a script or an agent can rely on

    0   success
    1   error: bad input, a missing object, I/O; the message is on stderr (with --json, as one JSON line)
    2   usage error: unknown command or option, a missing argument (printed by the argument parser)
    3   blocked: something you can provide is missing (an approval, a validation, a lease token);
        provide it and run the same command again. The stream was not changed.
    4   refused: a conflict, a policy denial, or an invalid request. Waiting will not help;
        the stream was not changed.

Which commands use 3 and 4

    ag integrate, ag op resume, ag remote integrate   0 integrated, 3 blocked, 4 conflict / denied / invalid
    ag remote prepare                                 0 nothing blocks it, 3 blocked, 4 conflict or problem
    ag preview                                        0 clean, 4 conflict or problem
    ag sync, ag pull                                  0 up to date, 4 the merge left conflict markers
    ag validate run                                   0 the check passed, 4 it failed (the failure is recorded)
    ag queue run                                      0 everything integrated, 3 something is blocked or failed
    ag predict                                        0 no overlapping work, 3 overlaps found

Commands that find a problem with the repository itself exit 1: ag verify, ag audit, ag recover (errors),
ag backup (unhealthy copy), ag doctor (a check failed).

Branch on the code, not on message text. With --json, `ag integrate` also returns `outcome.status`
and stable `blockers[].code` values (see `ag help policy`).

Output goes to stdout; errors, hints and diagnostics go to stderr. `--quiet` removes confirmations and hints,
never errors.

More: docs/reference/cli.md
