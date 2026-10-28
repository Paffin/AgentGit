# ADR 0020: expiring executor connection reports

Status: implemented on the development branch; not in beta.3.

## Problem

Registering an agent key does not connect a process that can execute its tasks. The Web UI must distinguish an enabled identity from an executor that recently contacted the server, including before its first run.

## Decision

An operator can renew `agent.executor.report` for an enabled registered agent. The server validates the name, available/total slots and a 30–900 second lifetime. It stamps receive/expiry times with its own clock and stores one latest report per agent in repository metadata. Existing `agent.list` returns that optional report and whether it is still current. Expired reports remain visible for diagnosis; disabled agents never appear connected.

The Go scheduler prepares its executor identities before reporting: it creates/loads the local key and registers it, or verifies the existing public key. A key mismatch prevents a report. Reports renew at most once every 20 seconds and describe the most recent capacity snapshot. Reporting errors never grant permissions or bypass task claims. Reports omit commands, environment variables, model credentials and tokens. A new/reconnected executor produces an audit record; ordinary renewals do not flood the audit log.

The registered-agent view shows connection and last reported capacity separately from identity status. Assignment choices keep offline agents selectable, but label missing/expired reports. The UI uses the server-adjusted clock so an expired report cannot remain green between refreshes. Older servers and orchestrators retain their previous flow.

## Limits

This is an operator-reported connection, not a command health check or a guarantee of model availability. When multiple orchestrators report for one agent, only the latest report is shown; capacity is not aggregated. Browser-managed executor provisioning and durable stdout logs remain open work. No metadata schema migration is needed because this is additive diagnostic metadata, not an execution or authorization rule.
