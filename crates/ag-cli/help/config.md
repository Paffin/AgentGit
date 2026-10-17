Configuration: files and environment

`ag config list` shows every effective setting and where it came from.

Files

    .ag/config.json          the repository: namespace, default stream, storage backends (set by `ag init`)
    .ag/remotes.json         remotes (managed with `ag remote add|list|remove`); never holds a secret
    .ag/credentials/NAME     a remote's token stored with --token-stdin (mode 0600)
    .ag/keys/                agent and issuer private keys (mode 0600)
    .agignore                files the working directory scan skips (one glob per line)
    user file                personal defaults: `ag config path` prints where; keys: identity, color

    ag config set identity user:alice      store your default identity
    ag config set color never
    ag config unset color

Precedence, highest first

    identity    --as, AG_IDENTITY, the user file, user:$USER
    colour      --color WHEN, the user file (when not auto), NO_COLOR, CLICOLOR_FORCE, TERM=dumb /
                CLICOLOR=0, then whether the output is a terminal
    repository  -C DIR / --repo DIR, else the nearest parent directory that contains .ag/
    credential  the variable named by the remote's token_env, the remote's token file,
                AG_TOKEN_<REMOTE>, AG_TOKEN

Environment variables

    AG_IDENTITY      the acting identity
    AG_CONFIG        path of the user file
    AG_KEY_DIR       where private keys live instead of .ag/keys
    AG_RUN           the agent run a server credential acts for
    AG_TOKEN         credential for remotes; AG_TOKEN_<REMOTE> for one remote (upper-case, non-letters as _)
    NO_COLOR, CLICOLOR, CLICOLOR_FORCE, TERM    colour (https://no-color.org)

Storage backends are configured in `ag init --storage FILE` (PostgreSQL URL and S3 credentials are read from
the environment variables that file names; see docs/operations/postgres-and-s3.md).

More: docs/reference/configuration.md
