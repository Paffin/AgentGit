# Hardening a deployment

A checklist for the person who runs `ag-server`. It is short on purpose and every row can be checked: the
**Setting** and **Default** columns are compared with the real defaults of the server by a test
(`crates/ag-server/tests/operator_hardening.rs`), and every test or file named in **Evidence** has to exist - the
test fails when one is renamed or removed. What the page cannot promise is under [What this does not
cover](#what-this-does-not-cover).

Read it next to [Deploying `ag-server`](deployment.md) (how) and the [security model](../architecture/security.md)
(why). "Server enforces" means the server refuses to start or to serve; "you" means nothing stops you from
getting it wrong.

## 1. Network and TLS

| Setting | Default | What to do | Evidence |
|---|---|---|---|
| `server.listen` | `"127.0.0.1:8080"` | Leave it on loopback unless something in front of the server terminates TLS. Server enforces: on any other address it refuses to start without TLS | `plaintext_on_a_public_address_is_refused` |
| `server.allow_insecure_remote` | `false` | Set it only behind a TLS terminator, and keep the port off the public network. The container images' configuration sets it; the Kubernetes `NetworkPolicy` admits only the namespace and the Compose file publishes on `127.0.0.1` | `crates/ag-server/tests/deploy_configs.rs`, `deploy/kubernetes/base/networkpolicy.yaml`, `deploy/compose/compose.yaml` |
| `server.tls_cert` | `null` | With `server.tls_key`: HTTPS and gRPC share one certificate. Server enforces: both or neither | `tls_files_must_come_as_a_pair`, `tls_serves_https_and_grpc_with_the_same_certificate` |
| `server.tls_key` | `null` | see `server.tls_cert` | `tls_files_must_come_as_a_pair` |
| `server.cors_origins` | `[]` | List the exact origins of browser clients served from elsewhere. Server enforces: `scheme://host[:port]` only, `*` is refused | `values_that_would_misbehave_are_refused_at_start_up` |
| `server.trusted_proxies` | `[]` | List the addresses or CIDR ranges of the reverse proxies and load balancers in front of the server. Only a connection from one of them is believed when it names the client (`Forwarded`, else `X-Forwarded-For`, read from the right past trusted hops); everyone else's header is ignored, so a client cannot choose the address in the logs or the bucket of its failed logins. Empty: the peer of the connection is the client. Server enforces: an entry that is not an address or range is refused at start-up | `behind_a_trusted_proxy_the_client_address_is_logged_and_anyone_elses_header_is_not_believed`, `crates/ag-server/src/proxy.rs` |

## 2. Credentials

| Setting | Default | What to do | Evidence |
|---|---|---|---|
| `auth.bootstrap_token_env` | `"AG_BOOTSTRAP_TOKEN"` | The variable holds the break-glass administrator: at least 32 random characters (`openssl rand -base64 32`). Use it to create the first roles and credentials, then stop setting it; offline `ag-server admin mint` still works. Server enforces: fewer than 16 characters is refused at start-up, 16 to 31 is accepted | `a_bootstrap_token_that_is_too_short_to_be_a_secret_is_refused_at_start_up`, `crates/ag-server/tests/admin_config.rs` |
| `auth.max_agent_ttl_secs` | `3600` | Agent credentials live at most this long. Lower it if your agents' sessions are shorter. Server enforces on every mint | `credential_lifetimes_follow_one_rule_for_every_caller` |
| `auth.default_agent_ttl_secs` | `3600` | What a mint without a lifetime gets (capped by the maximum) | `credential_lifetimes_follow_one_rule_for_every_caller` |
| `auth.max_ttl_secs` | `2592000` | Everyone else's credentials: 30 days by default. Lower it for people who can sign in with OIDC | `credential_lifetimes_follow_one_rule_for_every_caller` |
| `auth.default_ttl_secs` | `86400` | see `auth.max_ttl_secs` | `credential_lifetimes_follow_one_rule_for_every_caller` |
| `auth.oidc` | `[]` | Prefer OIDC for people (short-lived tokens, groups mapped to roles with `group_roles`). Only RS256, ES256 and EdDSA are accepted unless you widen `algorithms`; issuer and audience are mandatory | `oidc_tokens_are_validated_strictly_and_groups_map_to_roles` |
| `control.bindings_ttl_ms` | `5000` | Several replicas cache role bindings for at most this long, so a revoked role is effective everywhere within it (usually at once). Server enforces a ceiling of 300000 | `crates/ag-server/src/config.rs` |

Agents: have the orchestrator hold the `delegator` role and mint **one credential per run**, bound to that run
(`scope.run_id`, ADR-0010); it then dies with the run. Do not hand an agent a long-lived, name-bound credential. Revoke
with `DELETE /v1/credentials/{id}`.
Evidence: `a_run_bound_credential_dies_with_its_run`, `a_delegator_mints_credentials_for_agents_and_only_for_agents`.

## 3. Limits and quotas

The limits keep one caller, or one repository, from using the whole server. Rate limits are per caller; a quota of
`0` means **unlimited**, which is the default for every per-repository quota - set them.

| Setting | Default | What to do | Evidence |
|---|---|---|---|
| `limits.requests_per_second` | `200` | Sustained requests per second per caller. Lower it for agents | `requests_are_rate_limited_per_caller_with_a_retry_hint` |
| `limits.burst` | `400` | Burst size; must be at least 1 when the rate is set | `requests_are_rate_limited_per_caller_with_a_retry_hint` |
| `limits.auth_failures_per_second` | `0` | Failed authentications per second from one client address (see `server.trusted_proxies`); beyond the burst, requests from that address get `rate_limited` before their credential is looked at, a good one included. 0: no limit. Turn it on when the server is reachable by anyone who should not be guessing tokens | `failed_logins_are_counted_per_client_address_and_the_header_decides_only_behind_a_trusted_proxy` |
| `limits.auth_failure_burst` | `20` | Burst of failed authentications per client address | `failed_logins_are_counted_per_client_address_and_the_header_decides_only_behind_a_trusted_proxy` |
| `limits.max_concurrent_requests` | `512` | More are shed with `unavailable`. Worst-case memory is about this times `limits.max_object_bytes` (the [security model](../architecture/security.md#known-limits-read-these)): size it to the host | `crates/ag-server/src/limits.rs` |
| `limits.max_total_object_bytes` | `0` | Per-repository default for total object bytes. Set it. Server enforces on uploads and on workspace edits alike | `workspace_edits_are_counted_and_limited_like_uploads`, `concurrent_workspace_edits_cannot_pass_the_quota_between_them` |
| `limits.repo_max_object_bytes` | `0` | Per-repository default for the largest single object | `repository_quotas_are_enforced` |
| `limits.max_open_proposals` | `0` | Per-repository default for open proposals | `repository_quotas_are_enforced` |
| `limits.repo_max_streams` | `0` | Per-repository default for streams | `repository_quotas_are_enforced` |
| `limits.repo_requests_per_second` | `0` | Per-repository rate per caller (0: only the server-wide limit) | `new_repositories_start_with_the_configured_quota_defaults` |

## 4. Metrics and logs

| Setting | Default | What to do | Evidence |
|---|---|---|---|
| `metrics.enabled` | `true` | Turn it off if nothing scrapes it | `a_disabled_metrics_endpoint_is_not_replaced_by_the_page` |
| `metrics.token_env` | `"AG_METRICS_TOKEN"` | **Set the variable it names**: `/metrics` then needs `Authorization: Bearer ...`. Without it the endpoint is open to anyone who can reach the port, and the server logs a warning at start-up when it listens beyond loopback | `metrics_need_the_bearer_token_when_one_is_set_and_the_start_up_warns_when_it_is_not` |
| `telemetry.log_format` | `"text"` | `"json"` for log collectors | `values_that_would_misbehave_are_refused_at_start_up` |
| `telemetry.otlp_endpoint` | `""` | Trace export is off by default. Server enforces: no credentials in the URL; put headers in the variable `telemetry.otlp_headers_env` names, which is never logged | `tracing_is_off_by_default_and_configuration_is_validated` |

## 5. Data at rest

| Setting | Default | What to do | Evidence |
|---|---|---|---|
| `server.data_dir` | `"./ag-data"` | Holds the control database (the hash of every credential, the role bindings) and, unless you configure PostgreSQL and S3, the repositories. Server enforces: it creates the directory `0700` and warns at start-up when it exists and other users can enter it. It does not change an existing directory's mode, and encrypts nothing | `the_data_directory_is_created_private_and_an_open_one_is_called_out` |
| `control.url` | `null` | Never put a password in the file: use `control.url_env`. Server enforces: a URL with a password is refused | `passwords_are_detected` |
| `control.url_env` | `null` | The variable holds the PostgreSQL URL; secrets stay in your secret manager | `crates/ag-server/src/config.rs` |

Keys (`ag` agent and issuer keys, a worker's issuer key) are `0600` files and `ag` refuses one that others can read
(`unix_files_are_0600_and_directories_0700_and_loose_files_are_refused`); they are **not encrypted**, so keep
`AG_KEY_DIR` out of environments that run untrusted code. Back the control database up with
`ag-server admin backup-control` and practise `restore-control` (`a_dump_restores_everything_into_sqlite_and_postgres_and_back`);
the [durability page](durability.md) covers repositories. A restore is not a verification: run `ag verify` on a
repository and `GET /v1/repos/{owner}/{name}/audit/verify` for the audit chain after it.

## 6. Agents and the execution plane

| Setting | Default | What to do | Evidence |
|---|---|---|---|
| `agents.run_stale_after_secs` | `900` | A running run without a heartbeat for this long is stale | `an_operator_reaps_stale_runs_their_leases_and_their_credentials` |
| `agents.reap_interval_secs` | `0` | `0` never: a crashed agent keeps its leases and its task until someone reaps. Set it (for example 60) so the server ends stale runs, with their credentials, by itself; every such action is audited as `system:reaper` | `the_server_reaps_stale_runs_by_itself_when_it_is_told_to` |
| `agents.min_stale_after_secs` | `30` | The shortest threshold a reap request may name: a typo must not end every live run | `reap_runs_works_offline_and_says_what_it_did` |

Run `ag-worker` (the execution plane that runs checks on untrusted code) **on a host that is not the server**, with
the issuer key only there: whoever reads it can forge validations. The sandbox shares the host kernel; what it
stops and what it does not is on [its page](../architecture/sandbox.md).

## 7. Containers and Kubernetes

What the shipped files set, and where CI applies them (the [`Containers` workflow](https://github.com/Paffin/AgentGit/blob/main/.github/workflows/containers.yml)):

* the images are distroless, run as uid `65532`, with no shell (`deploy/docker/Dockerfile`); the data directory in the
  server image is `0700`;
* Compose: read-only root file system, all capabilities dropped, `no-new-privileges` (`deploy/compose/compose.yaml`);
* Kubernetes: `runAsNonRoot`, a `RuntimeDefault` seccomp profile, no privilege escalation, a read-only root file system,
  all capabilities dropped, and a `NetworkPolicy` (`deploy/kubernetes/base/deployment.yaml`,
  `deploy/kubernetes/base/networkpolicy.yaml`); the `restricted` Pod Security level is applied to a kind cluster by CI.

## 8. If you build or fork it

[Supply chain and vulnerability gates](supply-chain.md): which scanners run, how an accepted finding is recorded and
expires, SHA-pinned actions, `THIRD_PARTY_LICENSES.md`, SBOMs. Run `cargo deny check` and the scanners on your fork.

## What this does not cover

* **Encryption at rest.** Neither the control database nor the object store is encrypted by AgentGit; use disk or volume
  encryption, and treat a backup like the data.
* **Client certificates (mTLS).** Authentication is by bearer token or OIDC token only.
* **A hostile network between you and the TLS terminator.** With `allow_insecure_remote` the hop is plaintext.
* **The audit log is tamper-evident, not tamper-proof.** Anchor the head hash somewhere the server cannot write
  ([security model](../architecture/security.md)).
* **Real identity providers.** The OIDC code is exercised against an in-process issuer, not against a production IdP.
* **Everything in the security model's known limits**, which this page does not repeat.
* **No external audit** has been done.
