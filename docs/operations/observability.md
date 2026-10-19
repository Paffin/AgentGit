# Observability

`ag-server` exports **metrics** (Prometheus), **traces** (OpenTelemetry, OTLP/HTTP) and **structured logs**.
The files for all three are in [`deploy/observability/`](../../deploy/observability); the Compose stack
wires them up under `--profile observability`.

## Metrics

`GET /metrics` (Prometheus text format), served on the API port and protected by a bearer token when
`AG_METRICS_TOKEN` is set (always set it in production). Disable with `[metrics] enabled = false`.

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `agwp_requests_total` | counter | `transport`, `operation`, `code` | requests by result. `code` is `ok` or the [AGWP error code](../protocol/agwp-v1.md#errors) |
| `agwp_request_duration_seconds` | histogram | `transport`, `operation` | latency, 1 ms to 60 s |
| `agwp_inflight_requests` | gauge | | requests being handled |
| `agwp_auth_failures_total` | counter | `reason` | failed authentications (`missing`, `invalid`, `throttled`: the client address failed too often, see `limits.auth_failures_per_second`; and `run_ended`: a credential bound to a run that is no longer running, see [ADR-0010](../adr/0010-run-bound-credentials.md)) |
| `agwp_rate_limited_total` | counter | | requests rejected by the rate limiter |
| `agwp_shed_total` | counter | | requests rejected because the server was saturated |
| `agwp_integrations_total` | counter | `outcome` | `integrated`, `blocked`, `conflict`, `denied`, `invalid` |
| `agwp_object_bytes_received_total`, `agwp_object_bytes_sent_total` | counter | | object traffic |
| `agwp_open_repositories`, `agwp_event_subscriptions` | gauge | | |
| `agwp_job_events_total` | counter | `event` | execution plane since start: `enqueued`, `claimed`, `succeeded`, `retried`, `failed` (`retried` and `failed` include the jobs taken back when a worker died holding them: a runner that crashes on its last attempt shows up here, not only a runner that reports the failure) |
| `agwp_jobs_queued` | gauge | | queued validation jobs in repositories known to have work (counted, not listed, at scrape time; a repository that cannot be read at that moment is logged and left out of the sum) |
| `agwp_agent_runs_reaped_total` | counter | | agent runs ended as `abandoned` by a reap (an operator's `ReapRuns`, `ag-server admin reap-runs`, or the periodic reaper of `[agents] reap_interval_secs`) |
| `agwp_trace_spans_exported_total`, `agwp_trace_spans_dropped_total`, `agwp_trace_export_failures_total` | counter | | trace exporter health |
| `agwp_uptime_seconds` | gauge | | |

Label vectors only appear once they have been used (a fresh server has no `agwp_integrations_total` series).
Security-relevant detail: operation names and counts are visible to whoever can read `/metrics`, which is why
it takes a token.

**Alerts** ([`alerts.yml`](../../deploy/observability/alerts.yml)): server down, server-side error ratio,
p99 latency, load shedding, rate limiting, authentication-failure spikes, job backlog, permanently failing
jobs, trace export failing. Thresholds are starting points. For the Prometheus Operator the same rules are
generated into a `PrometheusRule` (`scripts/gen_prometheusrule.py`).

**Dashboard**: `grafana/dashboards/agentgit-overview.json` (request rate by result, error ratio, latency
percentiles and slowest operations, saturation, authentication failures, integrations by outcome, jobs,
object traffic, trace export). It is provisioned automatically in the Compose stack and can be imported
anywhere (datasource uid `agentgit-prometheus`).

## Traces

Set the collector and the server records **one `SERVER` span per handled operation**, named after the
operation (`proposals.create`, `integration.integrate`, …):

```toml
[telemetry]
otlp_endpoint = "http://otel-collector:4318"   # OTLP/HTTP, JSON encoding; empty = off
service_name = "agentgit-server"
otlp_sample_ratio = 1.0                         # fraction of NEW traces recorded
otlp_headers_env = "OTEL_EXPORTER_OTLP_HEADERS" # name=value,name2=value2 — a secret, never logged
```

* **Propagation.** An incoming W3C `traceparent` is continued: same trace id, the caller's span as parent.
  If the caller's `sampled` flag is off, nothing is recorded. Without a `traceparent` a new trace starts and
  `otlp_sample_ratio` applies.
* **Logs.** Every log line written while a request is served carries, in its `request{...}` span, `request_id`,
  `transport`, `remote` (the peer address of the connection - the proxy's, behind one) and `trace_id` (the
  exported trace's id, or the caller's own if nothing is exported for it, including a `sampled=0` trace), so a
  log line can be found from a trace and the other way round. A failed authentication logs the `remote` too.
* **Attributes.** `agwp.operation`, `agwp.transport`, `agwp.result` (`ok` or the error code),
  `agwp.request_id` and, if the client set one, `agwp.deadline_ms`. Credentials, request bodies and identities
  are never recorded. A span is marked as an error only for server-side faults (`internal`, `unavailable`,
  `deadline_exceeded`, `unimplemented`), not for a caller's `not_found` or `permission_denied`.
* **Never in the way.** Spans go through a bounded queue (4096) to one background thread that batches them
  (256 spans or 2 s) and POSTs `…/v1/traces`. If the queue is full or the collector is down, spans are
  dropped and counted; requests are unaffected. The queue is flushed on graceful shutdown (SIGTERM).
* **Limits.** The exporter is hand-written and small: JSON over HTTP only (no gRPC, no protobuf, no
  retries, no compression, no metrics or logs signals), and spans cover the request as the server sees it —
  there are no child spans for storage or engine calls yet. Use a collector in front for fan-out, retries
  and authentication to hosted backends. It is tested against a fake collector, not against a specific
  backend (Tempo, Jaeger, a vendor).

`ag-server`'s own log lines carry the same `request_id` as the span and the `X-Request-Id` response header,
which is the quickest way from a trace to a log line and back.

## Logs

JSON lines on stdout with `log_format = "json"` (the container configs set it), text otherwise;
`log_filter` is a `tracing` directive (`info,ag_server=debug`), or set `RUST_LOG`. Failed requests are logged at
`debug` with the operation, error code and request id; the audit log (`ag audit`, `GET …/events`) is the
durable record of what changed and who changed it.

## Health

| Endpoint | Meaning | Use for |
|---|---|---|
| `GET /healthz` | the process is up | liveness |
| `GET /readyz` | the control database answers | readiness / start-up |
| `ag-server healthcheck [--url …]` | the same probe as an exit code | container `HEALTHCHECK` (distroless has no curl) |
