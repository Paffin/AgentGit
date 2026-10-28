# ADR 0021: browser preparation of a task executor

Status: implemented in development; not in beta.3.

## Decision

A repository administrator can prepare an executor from the registered-agent view.
The wizard grants the chosen agent a writer role, creates a unique service identity
with the repository's delegator role and mints a 24-hour exact-repository token whose
operation scope is the task-only orchestrator's required set. It cannot integrate,
approve changes or administer roles. Review, advisory, index and validation schedulers
are disabled in this starting profile.

The JSON profile and token are separate downloads. The native `-token-file` option
loads `AG_ORCH_TOKEN` without putting the secret on the command line or in JSON;
Unix file permissions, type and length are checked. `config` and `ops` do not load
the credential. Local state is separated by repository and agent; the local control
listener gets an available loopback port.

The UI confirms only a fresh executor report from the unique identity minted in
this setup. An older process with the same agent name cannot satisfy the confirmation.
Changing accounts discards secrets and pending responses. Revocation removes the
credential and the setup service's delegator binding; it does not remove the agent's
writer role, which may predate setup or be used by another installation.

## Boundaries

An agent program and the orchestrator must be installed on an operator-controlled
host. The wizard neither installs software nor supplies a coding model. Downloaded
tokens remain valid when the dialog is closed or the in-memory copy is forgotten.
Actual model availability, retained stdout and fleet provisioning are separate work.

## Verification

The browser scenario downloads both files, verifies scope separation, runs the real
native check and orchestrator when AG_ORCH_BIN is provided, observes connection,
completes an assigned task and revokes the token. Local Chromium and phone runs use
that native path; CI without the native binary checks downloads, scope and revocation.
