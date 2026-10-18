"""An agent run with a signed provenance record, leases, tasks and conflict prediction - against a real server."""

from __future__ import annotations

import time
from typing import Any

import pytest

from agentgit import (
    AgentGitClient,
    CallOptions,
    FailedPreconditionError,
    InvalidArgumentError,
    LeaseConflictError,
    PermissionDeniedError,
    UnauthenticatedError,
    generate_agent_key,
    put_file,
    sign_provenance,
)
from tests.harness import RunningServer, start_server

REPO = "acme/agents"


@pytest.fixture(scope="module")
def team(server: RunningServer) -> dict[str, AgentGitClient]:
    admin = server.admin()
    admin.repos.create(name=REPO)
    team = {
        "admin": admin,
        "alice": server.actor("user:alice", REPO, "writer"),
        "bob": server.actor("user:bob", REPO, "reviewer"),
        "carol": server.actor("user:carol", REPO, "integrator"),
    }
    alice = team["alice"]
    alice.workspaces.create(repo=REPO, name="seed", stream="main")
    seed = alice.workspaces.modify(
        repo=REPO, name="seed", changes=[put_file("src/lib.rs", "fn a() {}\nfn b() {}\n")]
    )
    p = alice.proposals.create(repo=REPO, stream="main", head=seed.head, title="seed")
    assert team["carol"].integration.integrate(repo=REPO, proposal=p.id).outcome.status == "integrated"
    return team


def test_an_agent_run_lands_signed_work_under_policy(
    server: RunningServer, team: dict[str, AgentGitClient]
) -> None:
    admin, alice, bob, carol = team["admin"], team["alice"], team["bob"], team["carol"]
    admin.admin.policy_set(
        repo=REPO,
        stream="main",
        document='{"require_provenance_for_agents": true, "approvals": {"min": 0, "min_for_agents": 1}}',
    )
    bot = server.actor("agent:refactor-bot", REPO, "writer", scope={"repos": [REPO]})
    key = generate_agent_key()
    agent = bot.agents.register(repo=REPO, name="refactor-bot", public_key=key.public_key_hex)
    assert agent.identity == "agent:refactor-bot"
    with pytest.raises(PermissionDeniedError):  # an agent may only register itself
        bot.agents.register(repo=REPO, name="other-bot", public_key=key.public_key_hex)

    run = bot.agents.start_run(repo=REPO, agent="refactor-bot")
    assert run.identity == f"agent:refactor-bot#{run.run_id}"
    assert run.status == "running"

    # From here on every request is attributed to the run.
    runner = bot.with_run(run.run_id)
    runner.workspaces.create(repo=REPO, name="rename", stream="main")
    changed = runner.workspaces.modify(
        repo=REPO,
        name="rename",
        message="rename a to alpha",
        changes=[put_file("src/lib.rs", "fn alpha() {}\nfn b() {}\n"), put_file("NOTES.md", "notes\n")],
    )
    proposal = runner.proposals.create(repo=REPO, stream="main", head=changed.head, title="Rename a to alpha")
    assert proposal.author == run.identity

    # An agent cannot integrate on its own; and even a human integrator is blocked without provenance.
    with pytest.raises(PermissionDeniedError):
        runner.integration.integrate(repo=REPO, proposal=proposal.id)
    bob.proposals.review(repo=REPO, id=proposal.id, decision="approve")
    blocked = carol.integration.integrate(repo=REPO, proposal=proposal.id)
    assert blocked.outcome.status == "blocked"
    assert "missing_provenance" in [b.code for b in blocked.outcome.blockers]

    record: dict[str, Any] = {
        "changeset": changed.head,
        "agent": run.identity,
        "run_id": run.run_id,
        "model": "example-model",
        "rationale": "rename for clarity",
        "parameters": {"temperature": "0.2", "max_tokens": "4096"},
        "checks_run": ["lint"],
        "created_at_ms": int(time.time() * 1000),
    }
    no_retry = CallOptions(retry=False)
    # A signature over a different record is refused ...
    with pytest.raises(InvalidArgumentError) as tampered:
        runner.provenance.submit(
            repo=REPO,
            record={**record, "rationale": "something else"},
            signature=sign_provenance(record, key.private_key),
            options=no_retry,
        )
    assert tampered.value.status == 400
    # ... and so is one made with a key that was not registered for the agent.
    with pytest.raises(InvalidArgumentError):
        runner.provenance.submit(
            repo=REPO,
            record=record,
            signature=sign_provenance(record, generate_agent_key().private_key),
            options=no_retry,
        )

    # The agent's own signature is accepted: the server verifies exactly the bytes the SDK signed.
    entry = runner.provenance.submit(
        repo=REPO, record=record, signature=sign_provenance(record, key.private_key)
    )
    assert entry.verified
    assert entry.public_key == key.public_key_hex

    listed = alice.provenance.list(repo=REPO, changeset=changed.head)
    assert len(listed.entries) == 1
    assert listed.entries[0].verified
    assert listed.entries[0].record is not None
    assert listed.entries[0].record.model == "example-model"
    assert listed.entries[0].record.parameters == {"max_tokens": "4096", "temperature": "0.2"}

    done = carol.integration.integrate(repo=REPO, proposal=proposal.id)
    assert done.outcome.status == "integrated"
    assert alice.tree.file(repo=REPO, path="src/lib.rs", rev="main") == b"fn alpha() {}\nfn b() {}\n"

    assert runner.agents.heartbeat(repo=REPO, run_id=run.run_id).server_time_ms > 0
    assert (
        runner.agents.complete(repo=REPO, run_id=run.run_id, status="completed", summary="renamed").status
        == "completed"
    )
    assert alice.agents.list_runs(repo=REPO, agent="refactor-bot").runs[0].status == "completed"
    assert "refactor-bot" in [a.name for a in alice.agents.list(repo=REPO).agents]
    with pytest.raises(FailedPreconditionError):  # a finished run can no longer be used as an identity
        runner.workspaces.create(repo=REPO, name="late", stream="main")


def test_unicode_escapes_and_key_order_survive_the_signature_round_trip(server: RunningServer) -> None:
    bot = server.actor("agent:unicode-bot", REPO, "writer")
    key = generate_agent_key()
    bot.agents.register(repo=REPO, name="unicode-bot", public_key=key.public_key_hex)
    run = bot.agents.start_run(repo=REPO, agent="unicode-bot")
    runner = bot.with_run(run.run_id)
    runner.workspaces.create(repo=REPO, name="uni", stream="main")
    changed = runner.workspaces.modify(repo=REPO, name="uni", changes=[put_file("u.txt", "x")])
    record = {
        "changeset": changed.head,
        "agent": run.identity,
        "run_id": run.run_id,
        "rationale": 'quote " backslash \\ newline \n snowman ☃ crab \U0001f980 Привет',
        "inputs": ["line\r\nbreak", "\u2028"],
        "parameters": {"\U00010000": "astral", "\uffff": "bmp", "z": "1", "é": "2", "a-first": "3"},
        "created_at_ms": 1_760_659_200_000,
    }
    entry = runner.provenance.submit(
        repo=REPO, record=record, signature=sign_provenance(record, key.private_key)
    )
    assert entry.verified
    runner.agents.complete(repo=REPO, run_id=run.run_id, status="completed")


def test_exclusive_leases_refuse_each_other_and_fencing_tokens_only_increase(server: RunningServer) -> None:
    a1 = server.actor("agent:worker-a", REPO, "writer")
    a2 = server.actor("agent:worker-b", REPO, "writer")
    l1 = a1.leases.acquire(repo=REPO, scope="docs/**", exclusive=True, ttl_ms=60_000)
    assert l1.lease.exclusive
    with pytest.raises(LeaseConflictError) as conflict:
        a2.leases.acquire(repo=REPO, scope="docs/api/**", exclusive=True, ttl_ms=60_000)
    assert (conflict.value.status, conflict.value.retryable) == (409, False)

    soft = a2.leases.acquire(repo=REPO, scope="docs/api/**", ttl_ms=60_000)
    assert [x.id for x in soft.overlapping] == [l1.lease.id]

    with pytest.raises(FailedPreconditionError):  # only the holder releases or extends
        a2.leases.release(repo=REPO, lease_id=l1.lease.id)
    extended = a1.leases.extend(repo=REPO, lease_id=l1.lease.id, ttl_ms=120_000)
    assert extended.fencing_token == l1.lease.fencing_token
    assert extended.expires_at_ms > l1.lease.expires_at_ms
    assert len(a1.leases.list(repo=REPO, active_only=True).leases) >= 2
    a1.leases.release(repo=REPO, lease_id=l1.lease.id)

    l2 = a2.leases.acquire(repo=REPO, scope="docs/**", exclusive=True, ttl_ms=60_000)
    assert l2.lease.fencing_token > l1.lease.fencing_token


def test_tasks_form_a_dependency_graph(team: dict[str, AgentGitClient]) -> None:
    alice = team["alice"]
    extract = alice.tasks.create(repo=REPO, title="extract module", scope=["src/**"])
    use = alice.tasks.create(repo=REPO, title="use module", depends_on=[extract.id])
    assert not use.ready
    ready = [t.id for t in alice.tasks.list(repo=REPO, ready_only=True).tasks]
    assert extract.id in ready
    assert use.id not in ready

    with pytest.raises(FailedPreconditionError):  # cycles are refused
        alice.tasks.depend(repo=REPO, task=extract.id, depends_on=use.id)
    alice.tasks.finish(repo=REPO, id=extract.id, state="done")
    assert alice.tasks.get(repo=REPO, id=use.id).ready

    third = alice.tasks.create(repo=REPO, title="third")
    assert alice.tasks.depend(repo=REPO, task=third.id, depends_on=use.id).depends_on == [use.id]
    assert isinstance(alice.agents.predict(repo=REPO, stream="main").overlaps, list)


def test_credentials_bound_to_a_run_act_only_as_it_and_die_with_it() -> None:
    # A reap threshold of a second is for this test only: the server's minimum is 30 seconds unless told otherwise.
    server = start_server(config="[agents]\nmin_stale_after_secs = 1\n")
    try:
        admin = server.admin()
        admin.repos.create(name=REPO)
        orchestrator = server.actor("service:orchestrator", REPO, "delegator")
        bot = server.actor("agent:bound-bot", REPO, "writer")
        key = generate_agent_key()
        bot.agents.register(repo=REPO, name="bound-bot", public_key=key.public_key_hex)
        run_a = bot.agents.start_run(repo=REPO, agent="bound-bot")
        run_b = bot.agents.start_run(repo=REPO, agent="bound-bot")

        def bind(run_id: str) -> AgentGitClient:
            minted = orchestrator.credentials.mint(
                subject="agent:bound-bot", ttl_secs=300, scope={"repos": [REPO], "run_id": run_id}
            )
            assert minted.credential.scope is not None
            assert minted.credential.scope.run_id == run_id
            return server.anonymous(token=minted.token)

        ca, cb = bind(run_a.run_id), bind(run_b.run_id)
        me = ca.session.whoami()
        assert me.scope is not None
        assert me.scope.run_id == run_a.run_id

        # Each acts as its own run - the server attributes the work to it, with no `ag-run` header - and no other.
        ws = ca.workspaces.create(repo=REPO, name="bound-a", stream="main")
        assert ws.owner == f"agent:bound-bot#{run_a.run_id}"
        with pytest.raises(PermissionDeniedError):
            cb.agents.heartbeat(repo=REPO, run_id=run_a.run_id)
        with pytest.raises(PermissionDeniedError):
            cb.workspaces.modify(repo=REPO, name="bound-a", changes=[put_file("x", "x")])
        cb.agents.heartbeat(repo=REPO, run_id=run_b.run_id)

        # A delegator hands out agent credentials and nothing else.
        with pytest.raises(PermissionDeniedError):
            orchestrator.credentials.mint(subject="user:alice", ttl_secs=60, scope={"repos": [REPO]})
        with pytest.raises(PermissionDeniedError):
            orchestrator.credentials.mint(
                subject="agent:bound-bot",
                ttl_secs=60,
                scope={"repos": [REPO], "operations": ["integrate.run"]},
            )

        # The operator ends run a on the agent's behalf: its credential stops working at once; run b's does not.
        orchestrator.agents.complete(repo=REPO, run_id=run_a.run_id, status="failed", summary="session lost")
        with pytest.raises(UnauthenticatedError):
            ca.session.whoami()
        me_b = cb.session.whoami()
        assert me_b.scope is not None
        assert me_b.scope.run_id == run_b.run_id

        # Run b goes silent; the operator reaps it.
        time.sleep(1.2)
        reaped = orchestrator.agents.reap_runs(repo=REPO, stale_after_ms=1000)
        assert [(r.run_id, r.status) for r in reaped.runs] == [(run_b.run_id, "abandoned")]
        assert reaped.revoked_credentials == 1
        with pytest.raises(UnauthenticatedError):
            cb.session.whoami()
        with pytest.raises(PermissionDeniedError):  # a writer is no operator
            bot.agents.reap_runs(repo=REPO, stale_after_ms=1000)
    finally:
        server.stop()
