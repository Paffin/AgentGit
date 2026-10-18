Machine-readable output: --json

Every command that reports something accepts the global `--json` flag (before or after the command):

    ag --json log
    ag integrate PROPOSAL --json

Standard output then holds exactly one JSON document and nothing else; text, hints and diagnostics go to
standard error. Errors become one JSON line on standard error:

    {"error":{"message":"not found: proposal 'x'","hint":"list proposals with `ag proposal list` ...","exit_code":1}}

Commands that print file contents or a template are the exceptions, because their output is the data: `ag cat`,
`ag remote cat`, `ag policy example`, `ag completions`, `ag man`. `ag help` prints text.

A command that never ends is the other exception to "one document": `ag remote events --follow --json` prints one
compact JSON object per line (NDJSON), so that a script can read it line by line.

Stability: the shapes are version 1 (`ag version --json` prints `json_schema`). Within a version, keys are only
ever added, never removed, renamed or retyped, so ignore keys you do not know. The shapes are checked by golden
tests; the list of commands and their shapes is docs/reference/cli-json.md.

Typical use in a script:

    id=$(ag --json propose -t "Fix typo" | jq -r .id)
    ag integrate "$id" --json; rc=$?       # branch on the exit code, see `ag help exit-codes`
