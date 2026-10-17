"""Errors, retries, idempotency keys and deadlines.

AGENTGIT_URL=http://127.0.0.1:8080 AGENTGIT_TOKEN=... python examples/errors.py
"""

import os
import time

base_url = os.environ["AGENTGIT_URL"]
token = os.environ["AGENTGIT_TOKEN"]
repo = f"demo/errors-{int(time.time() * 1000)}"

# region errors
from agentgit import (
    AgentGitClient,
    AgentGitError,
    CallOptions,
    DeadlineExceededError,
    IdempotencyKeyReusedError,
    NotFoundError,
    RetryPolicy,
)

client = AgentGitClient(
    base_url,
    token,
    request_timeout_ms=10_000,  # per attempt; sent to the server as Request-Timeout
    retry=RetryPolicy(max_attempts=4),  # exponential backoff with jitter; honours Retry-After
)

try:
    client.proposals.get(repo="demo/does-not-exist", id="prop_1")
except NotFoundError as error:  # every stable error code has its own subclass of AgentGitError
    print(error.code)  # "not_found": a stable, machine-readable code
    print(error.status)  # 404
    print(error.retryable)  # False: the SDK did not retry it, and you should not
    print(error.request_id)  # the id to quote when asking an operator for the server log

# Mutations that the server can deduplicate carry an Idempotency-Key that is generated for you and
# reused by every retry, so a lost response never creates the repository twice. Pass your own key
# to make a retry of the whole program safe too.
client.repos.create(name=repo, options=CallOptions(idempotency_key=f"create-{repo}"))
client.repos.create(name=repo, options=CallOptions(idempotency_key=f"create-{repo}"))  # the recorded result

# Reusing a key for a different request is an error, not a retry.
try:
    client.repos.create(name=f"{repo}-other", options=CallOptions(idempotency_key=f"create-{repo}"))
except IdempotencyKeyReusedError as error:
    print(error.code)  # "idempotency_key_reused"

# A deadline can expire while the server is still working. The error says so: ask for the outcome
# before repeating anything that is not idempotent.
try:
    client.streams.head(
        repo=repo,
        stream="main",
        wait_version_gt=1000,
        wait_ms=20_000,
        options=CallOptions(timeout_ms=300, retry=False),
    )
except DeadlineExceededError as error:
    print(isinstance(error, AgentGitError), error.outcome_unknown)  # True True
# endregion errors

# What the test checks.
errors_result = {"repo": repo}
