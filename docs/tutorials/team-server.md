# Run a team server

A local repository is complete on its own. A **server** is how several people, agents and CI systems share one
repository: changes travel to it as proposals, and they land there under *its* policy, approvals and validations,
whatever anyone's laptop says. In this tutorial you start a real `ag-server` on your machine, give a repository to
four identities with different roles, and watch the server refuse what the roles do not allow.

Everything below is run by the test suite against the real `ag` and `ag-server` binaries
([how](README.md#how-these-tutorials-are-tested)); the server listens on a loopback port. For a server other people
can reach, read [Deploying `ag-server`](../operations/deployment.md) afterwards: TLS, secrets, containers, Kubernetes.

You need `ag` and `ag-server` ([install](../getting-started.md#install)), and a shell.

<!-- doctest-port 7878 -->
<!-- doctest-needs ag-server -->

## 1. Configure the server and create the repository

The server reads one TOML file. These two keys are all this tutorial changes; `ag-server default-config` prints every
setting with its default. The data directory holds the control database (credentials, roles, the list of repositories)
and the repositories.

<!-- doctest -->
```bash
mkdir team && cd team
cat > ag-server.toml <<'TOML'
[server]
listen = "127.0.0.1:7878"
data_dir = "./ag-data"
TOML
ADMIN="ag-server admin --config ag-server.toml"
$ADMIN create-repo acme/web
```

<!-- expect -->
```text
created acme/web (namespace acme/web)
```

The `admin` subcommand works directly on the data directory, also while the server is stopped; it is how the very
first administrator is made. (A running server also takes the same operations over HTTP, authenticated with a
bootstrap token from the environment variable named by `auth.bootstrap_token_env`; see the
[deployment guide](../operations/deployment.md#secrets).)

## 2. Roles and credentials

Access is a **role** on a repository, for an identity or a pattern of identities. The roles are `reader`, `writer`,
`reviewer`, `integrator`, `runner` and `admin`; each allows a set of operations, and more than one can be granted.
We make alice an administrator who writes, bob a writer who also reviews and integrates, carol a reader, and give
the agent `refactor-bot` (section 7) the writer role. Roles granted with the offline tool while the server runs are
seen after `control.bindings_ttl_ms` (5 seconds by default), so we grant them before starting it.

<!-- doctest -->
```bash
$ADMIN grant --subject user:alice --repo acme/web --role admin
$ADMIN grant --subject user:alice --repo acme/web --role writer
$ADMIN grant --subject user:bob --repo acme/web --role writer
$ADMIN grant --subject user:bob --repo acme/web --role reviewer
$ADMIN grant --subject user:bob --repo acme/web --role integrator
$ADMIN grant --subject user:carol --repo acme/web --role reader
$ADMIN grant --subject agent:refactor-bot --repo acme/web --role writer
ALICE=$($ADMIN mint --subject user:alice --ttl 3600 2>/dev/null)
BOB=$($ADMIN mint --subject user:bob --ttl 3600 2>/dev/null)
CAROL=$($ADMIN mint --subject user:carol --ttl 3600 2>/dev/null)
```

<!-- expect -->
```text
granted admin to user:alice on acme/web
granted writer to user:alice on acme/web
granted writer to user:bob on acme/web
granted reviewer to user:bob on acme/web
granted integrator to user:bob on acme/web
granted reader to user:carol on acme/web
granted writer to agent:refactor-bot on acme/web
```

`mint` prints a **credential** (a bearer token that starts with `agt_`) **once**: the server stores only its hash. We
captured each token in a shell variable; in real life you hand it to its owner over a channel you trust. Credentials
expire (here after an hour), and can be restricted to repositories, streams and operations
(`mint --repo … --stream … --operation …`).

## 3. Start the server and clone

<!-- doctest -->
```bash
ag-server serve --config ag-server.toml > server.log 2>&1 &
until ag-server healthcheck --url http://127.0.0.1:7878 > /dev/null 2>&1; do sleep 0.2; done
URL=http://127.0.0.1:7878/acme/web
echo "$ALICE" | ag clone --token-stdin $URL alice-web
echo "$BOB" | ag clone --token-stdin $URL bob-web
echo "$CAROL" | ag clone --token-stdin $URL carol-web
ag -C alice-web remote whoami
ag -C carol-web remote whoami
```

<!-- expect -->
```text
Cloned acme/web into alice-web (0 changeset(s), 0 blob(s)); stream 'main' at (empty).
Cloned acme/web into bob-web (0 changeset(s), 0 blob(s)); stream 'main' at (empty).
Cloned acme/web into carol-web (0 changeset(s), 0 blob(s)); stream 'main' at (empty).
user:alice (user, via token)
roles: acme/web:admin, acme/web:writer
user:carol (user, via token)
roles: acme/web:reader
```

`healthcheck` is the probe that container health checks use (`/readyz`). `--token-stdin` stored each token in that
clone's `.ag/credentials/origin` (mode 0600) so it does not have to live in your environment; `remote.origin` in
`.ag/remotes.json` records the URL and never holds a secret. The identity comes **from the credential**, not from what
you claim: the server answered "alice" because the token is alice's. (Other ways to hand over a token, and TLS with a
private CA: [Working with a server](../guides/remote.md#connecting).)

## 4. Put a rule on the stream

Policy lives on the server, per stream. alice, as an admin, requires one approval that is not the author's own.

<!-- doctest -->
```bash
cat > policy.json <<'JSON'
{ "approvals": { "min": 1, "forbid_self_approval": true } }
JSON
ag -C alice-web remote policy main --set policy.json > /dev/null
ag -C alice-web remote streams
```

<!-- expect -->
```text
main                 ...  v0  policy v1
```

## 5. Propose, review, integrate

alice edits and pushes. `push` uploads only the objects the server lacks and opens a **proposal**; the stream does not
move.

<!-- doctest -->
```bash
printf '# web\n' > alice-web/README.md
ag -C alice-web --as user:alice snapshot -m "Add README"
P=$(ag -C alice-web --as user:alice -q push -t "Add README")
ag -C alice-web remote integrate "$P"     # expect-exit 3
```

<!-- expect -->
```text
Snapshot ... in workspace 'default'
Blocked (policy v1); the stream was not changed:
  - approvals (default): 0 of 1 required
```

alice is an administrator, so the *server* lets her ask to integrate; the *policy* says the change is not ready:
exit 3, "blocked", because an approval is something that can be provided. Roles decide who may ask; policy decides
what must be true. alice reviews her own change, which the policy does not count (`forbid_self_approval`); bob's
review does. `prepare` is the question "what would integrating do now?" without doing it.

<!-- doctest -->
```bash
ag -C alice-web remote review "$P" --approve > /dev/null
ag -C bob-web remote prepare "$P"         # expect-exit 3
ag -C bob-web remote review "$P" --approve -m "looks good"
ag -C bob-web remote integrate "$P"
ag -C alice-web pull
ag -C alice-web log
```

<!-- expect -->
```text
...
blocked: approvals (default): 0 of 1 required
What to do:
  ag remote review prop_... --approve
Recorded 'approve' on head ...
Integrated (fast_forward): stream is now at ... (version 1), 1 file(s) changed.
...
    Add README
```

`pull` brought the integrated result back into alice's clone (here it already had the same head, so there is nothing to
merge). Her clone mirrors the server's `main`; **do not run `ag integrate` in a clone**: landing happens on the server.

## 6. Roles are enforced where it matters

carol can read but not write, and the server says so. Her `pull` works; her `push` is refused before any object is
stored.

<!-- doctest -->
```bash
ag -C carol-web pull
cat carol-web/README.md
printf 'x\n' > carol-web/x.txt
ag -C carol-web --as user:carol snapshot -m "carol tries"
ag -C carol-web push -t "carol tries"     # expect-exit 1
```

<!-- expect -->
```text
Fast-forwarded the workspace to ...
# web
Snapshot ... in workspace 'default'
error: permission_denied: this identity (user:carol) may not perform 'objects.write' on 'acme/web' (roles: reader)
```

## 7. Add an agent

An agent is an identity like the others, with its own **short-lived, narrowly scoped** credential. The server caps the
lifetime of agent credentials (`auth.max_agent_ttl_secs`, an hour by default). This one may only act on `acme/web`, and
has the `writer` role: it can push proposals and cannot integrate them, whatever the policy says.

<!-- doctest -->
```bash
BOT=$($ADMIN mint --subject agent:refactor-bot --ttl 900 --repo acme/web 2>/dev/null)
echo "$BOT" | ag clone --token-stdin $URL bot-web
ag -C bot-web pull
printf '# web\n\nA demo.\n' > bot-web/README.md
ag -C bot-web --as agent:refactor-bot snapshot -m "Describe the project"
Q=$(ag -C bot-web --as agent:refactor-bot -q push -t "Describe the project")
ag -C bot-web remote integrate "$Q"       # expect-exit 1
ag -C bob-web remote prepare "$Q"         # expect-exit 3
```

<!-- expect -->
```text
Cloned acme/web into bot-web ...
...
Snapshot ... in workspace 'default'
error: permission_denied: this identity (agent:refactor-bot) may not perform 'integrate.run' on 'acme/web' (roles: writer)
strategy: fast_forward
candidate tree: ...
blocked: approvals (default): 0 of 1 required
```

How to give such an agent leases, provenance and an autonomy envelope is the subject of
[Your first agent-safe repository](first-agent-safe-repo.md); the same policy documents work on a server.
For the agent's own tool interface see the [MCP server](../guides/mcp.md) and the [SDKs](../guides/sdks.md).

## 8. Watch what happened

Every decision above is an event of the repository: the audit log, replayed over the API (and, with `--follow`,
streamed live: that is what the Web UI and the orchestrator subscribe to).

<!-- doctest -->
```bash
ag -C alice-web remote events --topics policy,proposal,stream | cut -c1-60
```

<!-- expect -->
```text
...  policy.set  ...
...  proposal.create  ...
...  proposal.review  ...
...  proposal.review  ...
...  stream.integrate  ...
...  proposal.create  {"actor":"agent:refactor-bot"...
```

The server shuts down when this page's shell ends (in a terminal you would stop it with Ctrl+C: it finishes the requests
in flight first).

## Where to go next

- **Operate it**: [Deploying `ag-server`](../operations/deployment.md) (TLS, secrets, Compose, Kubernetes),
  [Observability](../operations/observability.md), [Durability and backup](../operations/durability.md),
  [PostgreSQL and S3](../operations/postgres-and-s3.md).
- **Run checks on untrusted code**: [Execution-plane workers](../guides/workers.md).
- **See it in a browser**: the server serves the [Web UI](../guides/web-ui.md) on the same port when it was built with it
  (the container image is; a plain `cargo build` prints a page that says how to build it).
- **What the server can and cannot do today** is in [Working with a server](../guides/remote.md#limits-today).
