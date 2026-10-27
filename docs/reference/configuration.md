# Configuration: files, environment, precedence

This is the long form of `ag help config` (the terminal copy is in [Help topics](help-topics.md#config)). It covers the
`ag` command line. The server has its own configuration file, described in
[Deploying `ag-server`](../operations/deployment.md); the worker and the MCP server take flags and environment variables
documented in the [Workers guide](../guides/workers.md) and in `ag-mcp --help`.

`ag config list` shows every effective setting and where it came from, for example:

```text
identity              user:alice  [AG_IDENTITY]
color                 auto  [default]
user_config           /home/alice/.config/agentgit/config.json  [default location]
repo.root             /work/hello  [discovered]
repo.namespace        me/hello  [/work/hello/.ag/config.json]
repo.storage.meta     sqlite  [/work/hello/.ag/config.json]
keys.dir              /work/hello/.ag/keys  [default location]
```

Scripts should use `ag config get KEY --json` or `ag config list --json` (shapes: [`--json` output](cli-json.md#ag-config)).

## Files

| Path | What it holds | Who writes it |
|---|---|---|
| `.ag/config.json` | the repository: format version, namespace, default stream, and the storage backends when they are not the default | `ag init` |
| `.ag/remotes.json` | the remotes: name, server URL, repository (`owner/name`), the name of the environment variable or the file with the token, the CA file. **Never a secret.** | `ag clone`, `ag remote add|remove` |
| `.ag/credentials/NAME` | one remote's token, stored with `--token-stdin` (mode 0600; `ag` refuses a file other users can read) | `ag clone`, `ag remote add` |
| `.ag/keys/` | agent and issuer private keys (mode 0600). Keep this directory out of untrusted environments. | `ag agent register`, `ag validate keygen` |
| `.agignore` | files the working-directory scan skips: one glob per line, `#` comments; a pattern without `/` matches at any depth | you |
| the user file | personal defaults (keys `identity` and `color`). `ag config path` prints where it is | `ag config set|unset` |

The user file is, in order of preference: the file named by `AG_CONFIG`, `$XDG_CONFIG_HOME/agentgit/config.json`,
`~/.config/agentgit/config.json`, and `%APPDATA%\agentgit\config.json` where `HOME` is unset (Windows). `ag config set identity
user:alice` and `ag config set color never` write it; `ag config unset color` removes a key. The repository's own
settings are shown by `ag config list` but are read-only there.

`.ag/meta.db` and `.ag/store/` are the repository's data, not configuration. See
[Durability & backup](../operations/durability.md).

## Precedence, highest first

| Setting | Order |
|---|---|
| identity | `--as`, `AG_IDENTITY`, the user file, `user:$USER` |
| colour | `--color WHEN`, the user file (when not `auto`), `NO_COLOR`, `CLICOLOR_FORCE`, `TERM=dumb` / `CLICOLOR=0`, then whether the output is a terminal |
| repository | `-C DIR` / `--repo DIR`, else the nearest parent directory that contains `.ag/` |
| a remote's credential | the variable named by the remote's `token_env`, the remote's token file, `AG_TOKEN_<REMOTE>`, `AG_TOKEN`, then the login saved by `ag login` (`credentials.json` beside the user file, readable by you only) |

## Environment variables read by `ag`

| Variable | Meaning |
|---|---|
| `AG_IDENTITY` | the acting identity: `user:alice`, `agent:bot#run_…`, `service:ci` |
| `AG_CONFIG` | path of the user file |
| `AG_KEY_DIR` | where private keys live instead of `.ag/keys` |
| `AG_RUN` | the agent run a server credential acts for (the credential must belong to that agent) |
| `AG_TIMEOUT` | the deadline of each call to a server, in seconds: a whole number greater than zero (default 120); a push sends its objects in batches, each with the full deadline |
| `AG_TOKEN` | credential for remotes |
| `AG_TOKEN_<REMOTE>` | credential for one remote (upper-case; characters that are not letters or digits become `_`) |
| `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, `TERM` | colour, see <https://no-color.org> |

Set by `ag` for the commands it runs, not read from you: `ag validate run` starts the check with an empty environment
plus `PATH`, `HOME` (the scratch directory), `AG_TREE_ID` (the candidate tree being validated) and `AG_PROPOSAL`
(the proposal). A check can use them to record what it looked at; they are not a security boundary
(`validate run` does **not** sandbox the command — see [Validation](../guides/validation.md) and the
[Workers guide](../guides/workers.md) for checks on untrusted code).

Build-time only (not read at run time): `AG_BUILD_COMMIT` and `AG_BUILD_DATE` stamp `ag --version` when building from
a source tree without `.git` ([Releasing](../operations/releasing.md)).

## Storage backends

`ag init` creates a SQLite metadata database and a local object store under `.ag/`. For PostgreSQL metadata and/or
S3-compatible objects pass a JSON file with the `storage` section:

```sh
ag init --namespace acme/app --storage storage.json
```

The file names the *environment variables* that hold the connection URL and the S3 credentials, never the secrets
themselves; the default variable for the PostgreSQL URL is `AG_PG_URL`. The format and the S3 settings are in
[PostgreSQL & S3](../operations/postgres-and-s3.md).

## See also

- [`ag help exit-codes`](help-topics.md#exit-codes) and [`--json` output](cli-json.md): what a script can rely on.
- [Completions and man pages](completions.md).
- [Remote guide](../guides/remote.md): connecting, credentials, TLS with a private CA.
