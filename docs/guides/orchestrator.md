# Running the Agent Platform (the orchestrator)

The orchestrator is a separate service that runs **next to** the AgentGit server and manages a fleet of
agents for you: it turns open tasks into agent sessions, keeps every open proposal prepared for validation,
suggests resolutions for conflicting changes, leaves automated review comments and keeps a small index of the
code's symbols. It is an ordinary AGWP client with its own identity and least-privilege credentials, so it
inherits the rule the whole system is built on: **it may propose, it can never land**. It does not integrate,
approve, change policy, grant roles or touch storage; the people and the stream's [policy](policy.md) decide
what lands, exactly as without it.

This guide takes you from nothing to a running service. The reference (every configuration key, the executor
contract, the security model, the tests) is in the
[service README](../../services/agent-orchestrator/README.md). Everything below was run against the
`ag-server` built from this repository; the [worked example](#6-a-worked-example) is a transcript of a real
run, and `services/agent-orchestrator/examples/worked-example.sh` reproduces it.

## 1. What you need

- A running `ag-server` that speaks AGWP v1 and has the execution-plane routes (this repository's), reachable
  over https (plain http is accepted only for loopback unless you say otherwise).
- Administrator access to the server, to create identities and mint credentials once.
- Go 1.24 to build the service (`go build -o orchestrator ./cmd/orchestrator` in `services/agent-orchestrator`),
  or the Dockerfile there (see [Docker](#9-docker)).

## 2. Identities and roles

The orchestrator is not one identity but several, so that what it does can be told apart in the audit trail
and limited separately:

| Identity | Role on the repository | Used for |
|---|---|---|
| `service:orchestrator` | `delegator` | scheduling: starting runs, taking leases, preparing proposals, **minting the short-lived credentials of agents** (a `delegator` mints for `agent:*` only) and finishing or reaping their tasks and runs |
| `service:reviewer` | `reviewer` | the automated reviews |
| `service:advisor` | `reviewer` | the conflict advice |
| `agent:<name>` | `writer` | the agents the orchestrator runs; one identity per executor |

The service identity needs the `delegator` role, not `admin`. A delegator holds a writer's capabilities plus
two more: `credentials.delegate` (mint, list and revoke credentials for **agents only** - never for a person, a
service or an administrator - and never more than the delegator itself could do) and `agent.operate` (finish
a task or a run on an agent's behalf, and reap runs whose supervisor is gone; each is audited under the
orchestrator). It cannot change policy, roles or repositories. On top of that the credential it actually
*uses* is scoped to a list of operations (next section) that excludes integrating, policy changes, reviewing,
stream operations and role management, and `orchestrator check` warns if the credential is broader than
that. A server that predates the role is reported by `check` as an error (it would need `admin`, which gives
the orchestrator far more than it uses).

On a server's data directory, offline (the server may be stopped):

```bash
ag-server admin create-repo acme/web
ag-server admin grant --subject service:orchestrator --repo acme/web --role delegator
ag-server admin grant --subject service:reviewer     --repo acme/web --role reviewer
ag-server admin grant --subject service:advisor      --repo acme/web --role reviewer
ag-server admin grant --subject agent:docs-bot       --repo acme/web --role writer
```

People keep their ordinary roles: whoever approves (a `reviewer`) and whoever integrates (an `integrator`)
are the humans of your process, and they are the only ones who move the stream.

## 3. Credentials

The service needs one credential per identity, each in its own environment variable. `orchestrator ops` tells
you exactly which operations each needs for *your* configuration, in a form you can paste into the mint
command:

```bash
orchestrator ops -config orchestrator.yaml -for orchestrator -flags
export AG_ORCH_TOKEN=$(ag-server admin mint --subject service:orchestrator --ttl 2592000 \
    --repo acme/web $(orchestrator ops -config orchestrator.yaml -for orchestrator -flags) | tail -n 1)
export AG_REVIEWER_TOKEN=$(ag-server admin mint --subject service:reviewer --ttl 2592000 \
    --repo acme/web $(orchestrator ops -config orchestrator.yaml -for reviewer -flags) | tail -n 1)
export AG_ADVISOR_TOKEN=$(ag-server admin mint --subject service:advisor --ttl 2592000 \
    --repo acme/web $(orchestrator ops -config orchestrator.yaml -for advisor -flags) | tail -n 1)
```

(`ag-server admin mint` prints the token once, on its last line. Use your secret store rather than shell
variables in production.) The credentials are restricted to the repositories you list and the operations the
configuration needs; the credentials of the agents it starts are *contained* in the orchestrator's own, which
is why the list is derived from your configuration. Each agent credential is also **bound to the run of its
session**: it acts only as that run, so two sessions of one agent (`slots` above 1) cannot act for each other,
and it stops working when the run ends or is reaped. They expire: the service warns at start-up when one has
less than three days left and logs an error when less than a day is left; renew by minting a new one and
restarting. The reviewer and advisor variables may be left out only if you are content for those components
not to work (they fall back to the orchestrator's credential, which cannot post reviews, and `check` says so).

## 4. Configure

One YAML file (JSON also works). Unknown keys are errors. Tokens are never in it. This is the file used by the
worked example, `services/agent-orchestrator/examples/orchestrator.yaml`:

```yaml
# Agent Platform for the acme/web repository (see docs/guides/orchestrator.md).
#
# Credentials are never written here. The file only names the environment
# variables that hold them (the token_env settings).
server:
  url: http://127.0.0.1:18090
  token_env: AG_ORCH_TOKEN               # service:orchestrator (default)
  reviewer_token_env: AG_REVIEWER_TOKEN  # service:reviewer (default)
  advisor_token_env: AG_ADVISOR_TOKEN    # service:advisor (default)

listen: 127.0.0.1:19090                  # admin and observability API
state_dir: /var/lib/orchestrator         # agent keys, semantic index, session work directories

log:
  format: text
  level: info

loop:
  reconcile: 5s                          # safety net under the event-driven loop

shutdown:
  mode: wait                             # let running sessions finish ...
  grace: 2m                              # ... for at most this long

repos:
  - name: acme/web
    stream: main
    schedule:
      slots: 3                           # sessions at once in this repository
      placement: avoid                   # defer tasks whose scope overlaps live work
      lease: soft                        # advertise the task scope while it runs
      attempts: 2
      backoff: 20s
      default_executor: docs-bot
      routes:
        - executor: shell-bot
          scope: "notes/**"
    review:
      max_files: 40
      max_lines: 1500

executors:
  # Built-in stand-in for an agent: edits a virtual workspace, signs provenance, opens a proposal.
  - name: docs-bot
    type: scripted
    heartbeat: 5s
    scripted:
      task_files: true                   # the task description carries the files to write
      model: scripted-demo

  # Any program. It gets AG_SERVER, AG_TOKEN, AG_RUN, AG_REPO, AG_TASK, AG_KEY_FILE, ... in its environment.
  - name: shell-bot
    type: command
    command: ["/bin/sh", "/opt/orchestrator/shell-bot.sh"]
    pass_env: [PATH]
    timeout: 5m
    grace: 10s
    heartbeat: 5s
    credential_ttl: 10m
```

Choices that matter:

- **Executors are the agents.** A `command` executor runs any program with a documented environment (the
  [executor contract](../../services/agent-orchestrator/README.md#executor-contract)); the `scripted`
  executor is a built-in stand-in that exercises the whole path without a model. `routes` send tasks to an
  executor by title (a regular expression) and/or scope (a glob); `default_executor` catches the rest.
- **One agent per executor, and prefer one session per agent.** The server binds a credential to an agent
  name, not to a run, so concurrent sessions of the same agent (`slots` above 1) could act as each other's
  run. `check` warns about it. If sessions must be isolated from each other, give each its own executor and
  agent.
- **Placement.** With `avoid` (the default) the scheduler defers a task whose scope overlaps a live lease,
  a running session, work that has not landed yet, a file touched by an open proposal, or an overlap the
  server's conflict prediction reports; `allow` ignores everything but exclusive leases. Deferred tasks are
  not failures: the reason is in `/status` and the task starts when the reason is gone.
- **Timeouts.** An agent credential lives at most one hour and cannot be renewed in a running process, so
  an executor's `timeout` is capped at 55 minutes.
- **Shutdown.** `wait` lets running sessions finish within `grace`; `cancel` stops them at once (SIGTERM,
  then SIGKILL of the whole process group after the executor's own `grace`). A cancelled session leaves its task
  with the agent, and a restarted orchestrator adopts it.
- **Admin API.** It listens on loopback by default. To expose it, set `listen` to a non-loopback address
  **and** `admin.token_env`: `/status`, `/metrics` and the index API then need that bearer token, and the
  service refuses to start without it.

## 5. Check, run, watch

```bash
orchestrator check -config orchestrator.yaml     # changes nothing; exit 1 if anything is wrong
orchestrator run   -config orchestrator.yaml     # SIGTERM / SIGINT shuts it down gracefully
```

`check` negotiates the protocol, authenticates all three credentials, compares their roles and scopes with
what the configuration needs, reads every repository and stream and looks for the executors' programs. Its
output is in the worked example below. In production wire `run` to your supervisor, `/readyz` to your
readiness probe (200 once the service is authenticated and every repository's event stream is connected) and
`/healthz` to liveness.

## 6. A worked example

The transcript below is a real run of `services/agent-orchestrator/examples/worked-example.sh` (the server and
the orchestrator on loopback; temporary paths, credentials and timestamps shortened or omitted, nothing else
edited). It sets up the repository above with a policy that says agents' changes need **signed provenance and
one human approval**, files three tasks (B depends on A; C is routed to the shell executor), and then plays the
developer, the reviewer and the integrator.

```text
$ ag-server admin create-repo acme/web
$ ag-server admin grant --subject service:orchestrator --repo acme/web --role delegator
  ... (alice writer, bob reviewer, carol integrator, service:reviewer, service:advisor,
       agent:docs-bot and agent:shell-bot as above)

$ ag-server admin mint --subject service:orchestrator --ttl 2592000 --repo acme/web $(orchestrator ops -for orchestrator -flags)
(three service credentials minted; not shown)

$ # policy: agents' changes need signed provenance and one human approval
{'stream': 'main', 'version': 1}

$ # tasks: B depends on A; C is for the shell executor
A=task_2ce9e887aa07372f0ce0
B=task_f8d135cf638b9923f748 (depends on A)
C=task_a1982c24b69a1cc8f591 (routed to shell-bot)

$ orchestrator check -config orchestrator.yaml
ok    server                   http://127.0.0.1:18090 speaks agwp.v1 (server 0.1.0)
ok    orchestrator             authenticates as service:orchestrator (token), roles acme/web:delegator
ok    orchestrator             the credential's scope covers the 30 operations this configuration needs
ok    orchestrator             the credential expires 2026-11-16T17:32:26Z
ok    reviewer                 authenticates as service:reviewer (token), roles acme/web:reviewer
ok    reviewer                 the credential's scope covers the 6 operations this configuration needs
ok    advisor                  authenticates as service:advisor (token), roles acme/web:reviewer
ok    advisor                  the credential's scope covers the 4 operations this configuration needs
ok    acme/web                 stream "main" at version 1
ok    executor docs-bot        built-in scripted executor, agent "docs-bot"
ok    executor shell-bot       command "/bin/sh" found, agent "shell-bot"
ok    state_dir                .../state is usable

$ orchestrator run -config orchestrator.yaml &
{ "not_ready_because": [], "ready": true }
```

Task A (docs) and task C (notes) do not overlap, so both start at once; B waits because it depends on A:

```text
$ # the orchestrator ran task A (and C, which does not overlap); B waits for A
prop_0849bf70bc8fc2d0ac7d agent:shell-bot#run_85346b9f544a16367ee5 task=c8f591 'Note from shell-bot'
prop_9b911d00157a8a10fc2c agent:docs-bot#run_ffa8b55567a1ee9192e7 task=2f0ce0 'Add the architecture overview'

$ curl http://127.0.0.1:19090/status  (excerpt)
tasks: {'in_progress': 2, 'open': 1}  deferred: []
prop_0849bf70bc8fc2d0ac7d agent:shell-bot mergeable=yes validated=na approved=no ready=False ['insufficient_approvals', 'missing_provenance']
prop_9b911d00157a8a10fc2c agent:docs-bot mergeable=yes validated=na approved=no ready=False ['insufficient_approvals']

$ # what the server records about A: its author and the signed provenance
author: agent:docs-bot#run_ffa8b55567a1ee9192e7
reviews: []
provenance: agent:docs-bot#run_ffa8b55567a1ee9192e7 model=scripted-demo verified=True
```

Each open proposal has the four states the orchestrator tracks independently (mergeable, validated,
approved, integrated). The agent's provenance is signed with the agent's own key and **verified by the server**.
`reviews: []` is the automated reviewers saying nothing, which is what they do for a clean change. The shell
executor's proposal already shows what policy will say: that executor does not sign provenance.

Now the people take over. The orchestrator is never in this part:

```text
$ # the integrator tries to land A without an approval: the policy says no
blocked ['insufficient_approvals']

$ # bob approves, carol integrates
approve
integrated fast_forward

$ # A is finished by the orchestrator, B becomes ready and runs
prop_46f5e1249c4aa7199d47 agent:docs-bot#run_48e7c2b4b3866de3c8f3 task=23f748 'Add the runbook'
prop_0849bf70bc8fc2d0ac7d agent:shell-bot#run_85346b9f544a16367ee5 task=c8f591 'Note from shell-bot'
integrated

$ # C came from the shell executor, which does not sign provenance: the policy blocks it
blocked ['missing_provenance']
```

Once A's proposal landed, the orchestrator finished task A as its owner, B became ready, ran, and its proposal
went through the same gates. C stays blocked until someone makes that executor sign its provenance (or the
policy changes), and the orchestrator has no way to get around that. Last, the by-products and the audit
trail:

```text
$ # the semantic index follows the stream
Greet func src/greet/greet.go:4
indexed files=1 symbols=1

$ # the audit trail: who moved the stream?
stream.integrate by user:carol
stream.integrate by user:carol
stream.integrate by user:carol

$ curl http://127.0.0.1:19090/metrics  (excerpt)
orchestrator_credentials_minted_total{repo="acme/web"} 5
orchestrator_sessions_finished_total{repo="acme/web",executor="docs-bot",outcome="completed"} 2
orchestrator_sessions_finished_total{repo="acme/web",executor="shell-bot",outcome="completed"} 1
orchestrator_sessions_started_total{repo="acme/web",executor="docs-bot"} 2
orchestrator_sessions_started_total{repo="acme/web",executor="shell-bot"} 1

$ kill -TERM $(cat orch.pid)
orchestrator stopped
```

Three `stream.integrate` events (the seed, A and B), all by the integrator; no service identity moved the
stream. The log of the run:

```text
level=INFO msg="orchestrator running" version=0.1.0 listen=127.0.0.1:19090 repos=1 executors=2
level=INFO msg="connected to the server"
level=INFO msg="session started" repo=acme/web executor=docs-bot agent=docs-bot run=run_ffa8… task=task_2ce9… attempt=1
level=INFO msg="session started" repo=acme/web executor=shell-bot agent=shell-bot run=run_8534… task=task_a198… attempt=1
level=INFO msg="session ended" repo=acme/web executor=docs-bot … outcome=completed exit=0 task_disposition=keep duration=546ms
level=INFO msg="work landed or was closed; task finished" repo=acme/web component=scheduler task=task_2ce9… state=done
level=INFO msg="session started" repo=acme/web executor=docs-bot agent=docs-bot run=run_48e7… task=task_f8d1… attempt=1
level=INFO msg="work landed or was closed; task finished" repo=acme/web component=scheduler task=task_f8d1… state=done
level=INFO msg="shutting down" mode=wait grace=2m0s
level=INFO msg=stopped
```

(Lines about the semantic index and the second session's end are omitted here for brevity.)

What this run does **not** show is covered by the integration tests in `services/agent-orchestrator/integration`:
overlapping tasks being deferred while a lease is held, a failing executor, a killed executor, the advisor
resolving two conflicting workspaces, the reviewers' findings, the execution scheduler working against real
validation jobs signed by a CI worker, and both shutdown modes.

## 7. Operating it

### Reading `/status`

`GET /status` (with the admin token when one is configured) shows, per repository, the scheduler (running
sessions, work waiting to land, deferred tasks **with the reason**, retries), each open proposal's four states
and blockers, the validation and integration queues, the advisor's and reviewers' recent records and the
index. The reasons a ready task is deferred are stable strings:

| Reason | Meaning | What ends it |
|---|---|---|
| `exclusive_lease` | someone holds an exclusive lease on an overlapping scope | the lease is released or expires |
| `foreign_lease` | someone else holds a soft lease on an overlapping scope (`avoid` only) | same |
| `running_task` | one of the orchestrator's own sessions works on an overlapping scope | that session ends |
| `pending_integration` | finished work on an overlapping scope has not landed yet | a person integrates or closes the proposal |
| `open_proposal` | an open proposal already changes a file in the scope | the proposal lands or is closed |
| `predicted_overlap` | the server's conflict prediction reports an overlap | the other work lands |
| `executor_busy`, `repo_busy` | no free slot (not a conflict) | a session ends |
| `retry_backoff` | waiting out the delay after a failed attempt | time |
| `start_failed`, `lease_conflict` | the server refused to start the session or take the lease; retried | usually transient: see the log |

(A task whose dependency has not landed is not deferred at all: it is simply not ready, because the server
says so.)

### When things go wrong

| You see | What happened | What to do |
|---|---|---|
| a task is `cancelled` and its run is `failed` | the executor failed on every attempt (`attempts`); the run summary has the exit code and the tail of its stderr | fix the agent; file the task again |
| a task stays `in_progress` with no session | the orchestrator was stopped while it ran (cancel mode) or crashed | start the orchestrator: it adopts the task, or finishes it if its proposal landed |
| a run is `failed` with "abandoned" | its supervisor was gone and the heartbeat went stale; the orchestrator completed it, which also released its leases | nothing; the task is retried |
| `check` says a credential's scope lacks operations | the credential was minted before the configuration changed | re-run `orchestrator ops` and mint a new one |
| the reviewer or advisor shows as an error in `check` | its variable is unset and the orchestrator's own credential cannot post reviews | mint the credential or disable the component |
| `/readyz` is 503 | not yet authenticated, or an event stream is down; the body says which | check the server, the credential and the network |
| many `rate_limited` retries in the metrics | the server limits every credential's request rate | lower `slots`, raise the reconcile interval, or raise the server's limit |

### Rotating and watching credentials

Service credentials do not renew themselves. The gauge named orchestrator_service_token_expiry_seconds (a Unix
time) is in `/metrics` for each identity; alert well before it. To rotate, mint new credentials, update the variables and
restart (shutdown `wait` mode lets running sessions finish first). Agent credentials are the service's own
business: minted per session, revoked when the session ends.

## 8. How it interacts with policy

The orchestrator makes no decision a policy would make. What you should say in the policy, because agents
now exist:

- `require_provenance_for_agents: true` makes the signed account of what an agent did a condition of
  landing. Agents the orchestrator runs sign with the key it keeps in the state directory (the same file
  format as the CLI's), and the server verifies the signature; the `scripted` executor does this, a `command`
  executor must (the key path is `AG_KEY_FILE`).
- `approvals.min_for_agents` makes a person approve what an agent proposes. The orchestrator's reviews are
  always comments; the policy's `approvers` decides who counts, and by default only people do.
- `required_validations` with `trusted_issuers` makes tests a condition. The orchestrator asks the server to
  queue the validation jobs (`enqueue` in `validations`) and shows where each proposal stands; **running** them
  and **signing the result** is the trusted CI worker's job, with a key the policy lists. See
  [Validation and attestations](validation.md).

## 9. Docker

`services/agent-orchestrator/Dockerfile` is a multi-stage build to a static binary in a distroless image that
runs as a non-root user. **It has not been built**: the machine it was written on has no Docker daemon. It
carries no shell, so `command` executors need a derived image with their programs, or the orchestrator runs
where they are. To reach the admin API from outside the container set `listen` to a non-loopback address and
`admin.token_env`.

## 10. Limits

- The advisor and the reviewers are deterministic heuristics, not a model: they are explainable and cheap,
  and they can miss things (a secret in an unusual shape) and flag harmless ones (use the inline allow marker
  or `allow_secrets`). The semantic index is a line-based extractor for Go, Rust, Python, TypeScript and
  JavaScript, not a language server.
- A very long agent session (over 55 minutes) is not supported: agent credentials cannot outlive an hour.
- Concurrent sessions of one agent are not isolated from each other (section 4).
- See the README's list of server behaviours the service works around, and of what has not been verified.
