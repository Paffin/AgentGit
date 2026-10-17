Identities: who is acting

An identity is `kind:name`. The kind is `user`, `agent`, `service` or `system`:

    user:alice            a person
    agent:refactor-bot    a registered agent (see `ag agent register`)
    agent:refactor-bot#run_7f3a...   one run of that agent (`ag agent start` prints it)
    service:ci            a trusted validation service

Rules that compare people (self-approval, lease ownership) compare the principal, the part before `#`, so an
agent cannot approve its own work by starting a new run.

Which identity `ag` uses, first match wins

    1. --as IDENTITY
    2. the AG_IDENTITY environment variable
    3. the `identity` setting:  ag config set identity user:alice
    4. user:$USER

`ag config get identity` shows the result and where it came from; `ag status` and `ag doctor` show it too.

In local mode an identity is a label, not authentication: anyone who can run `ag` in the repository can claim
any identity. Against clients you do not control, use `ag-server`, which authenticates every request and
enforces policy there (docs/architecture/security.md).

Agents: `ag agent register NAME`, then `ag agent start NAME` prints the identity to use for the run:

    export AG_IDENTITY='agent:NAME#run_...'

More: docs/concepts.md and docs/guides/agents.md
