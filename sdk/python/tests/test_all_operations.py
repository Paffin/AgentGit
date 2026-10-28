"""A tour of EVERY operation of the HTTP API through the SDK, against a real server. A recording
transport maps each request back to its OpenAPI operation id; the final test fails if any
operation of the OpenAPI document was not reached (or if the server answered "no such route" / 405 for one).
"""

from __future__ import annotations

import json
import os
import re
import time
from typing import Any

import httpx
import pytest

from agentgit import (
    NAMESPACE_METHODS,
    OPERATIONS,
    AgentGitClient,
    AsyncAgentGitClient,
    CallOptions,
    PutOp,
    blob_object,
    build_changeset,
    generate_agent_key,
    put_file,
    sign_attestation,
    sign_provenance,
)
from tests.faults import AsyncFaultyTransport, FaultyTransport
from tests.harness import REPO_ROOT, RunningServer

REPO = "tour/all"
CI_KEY = generate_agent_key()
MATCHERS = [
    (
        op,
        re.compile(
            "^"
            + re.sub(r"\\\{[^}]+\\\}", "[^/]+", re.sub(r"\\\{\\\*[^}]+\\\}", ".+", re.escape(op.route)))
            + "$"
        ),
    )
    for op in sorted(OPERATIONS.values(), key=lambda op: op.route.count("{"))
]


class Recorder(FaultyTransport):
    """Counts the requests per operation id (and notes routes the server did not know)."""

    def __init__(self) -> None:
        super().__init__()
        self.reached: set[str] = set()
        self.route_missing: list[str] = []

    def handle_request(self, request: httpx.Request) -> httpx.Response:
        response = super().handle_request(request)
        path = request.url.path
        for op, regex in MATCHERS:
            if op.http == request.method and regex.match(path):
                self.reached.add(op.id)
                if response.status_code in (404, 405) and op.response != "sse":
                    response.read()  # never read an event stream: it does not end
                    if response.status_code == 405 or b"no such route" in response.content:
                        self.route_missing.append(op.id)
                break
        return response


@pytest.fixture(scope="module")
def recorder() -> Recorder:
    return Recorder()


@pytest.fixture(scope="module")
def tour(server: RunningServer, recorder: Recorder) -> dict[str, AgentGitClient]:
    http = httpx.Client(transport=recorder)
    admin = server.admin(http_client=http)
    admin.repos.create(name=REPO)
    opts: dict[str, Any] = {"http_client": http}
    return {
        "admin": admin,
        "alice": server.actor("user:alice", REPO, "writer", **opts),
        "bob": server.actor("user:bob", REPO, "reviewer", **opts),
        "carol": server.actor("user:carol", REPO, "integrator", **opts),
        "ci": server.actor("service:ci", REPO, "runner", **opts),
        "bot": server.actor("agent:tour-bot", REPO, "writer", **opts),
    }


def attest(v: dict[str, Any]) -> dict[str, Any]:
    return sign_attestation(
        {
            "id": f"att_{os.urandom(10).hex()}",
            **v,
            "result": "pass",
            "environment_digest": "sha256:tour",
            "issuer": "service:ci",
            "issued_at_ms": int(time.time() * 1000),
        },
        CI_KEY,
    )


class Seed:
    head = ""


def test_session_repositories_roles_and_credentials(tour: dict[str, AgentGitClient]) -> None:
    admin = tour["admin"]
    admin.session.negotiate()
    # public, like negotiate: a server without identity providers offers the pasted credential only
    auth = admin.session.auth_config()
    assert (auth.mode, auth.providers) == ("token", [])
    assert admin.session.whoami().identity == "user:bootstrap-admin"
    assert admin.repos.describe(repo=REPO).name == REPO
    assert len(admin.repos.list(prefix="tour/").repos) == 1
    assert admin.repos.configure(repo=REPO, description="the tour").description == "the tour"

    assert admin.roles.grant(subject="user:viewer", repo=REPO, role="reader").role == "reader"
    assert any(b.subject == "user:viewer" for b in admin.roles.list(repo=REPO).bindings)
    admin.roles.revoke(subject="user:viewer", repo=REPO, role="reader")
    assert admin.roles.list(subject="user:viewer").bindings == []

    minted = admin.credentials.mint(subject="user:temp", ttl_secs=60, description="tour")
    assert minted.token.startswith("agt_")
    assert len(admin.credentials.list(subject="user:temp").credentials) == 1
    assert admin.credentials.revoke(credential_id=minted.credential.id).revoked


def test_policy(tour: dict[str, AgentGitClient]) -> None:
    admin = tour["admin"]
    doc = {
        "trusted_issuers": [{"identity": "service:ci", "public_key": CI_KEY.public_key_hex}],
        "required_validations": [{"check": "tests", "issuers": ["service:ci"], "run": {"argv": ["true"]}}],
        "approvals": {"min": 1},
    }
    assert admin.admin.policy_set(repo=REPO, stream="main", document=json.dumps(doc)).version == 1
    assert json.loads(admin.admin.policy_get(repo=REPO, stream="main").document)["approvals"]["min"] == 1
    assert len(admin.admin.policy_history(repo=REPO, stream="main").versions) == 1


def test_workspaces_proposals_reviews_and_the_execution_plane(tour: dict[str, AgentGitClient]) -> None:
    alice, bob, carol, ci = tour["alice"], tour["bob"], tour["carol"], tour["ci"]
    assert alice.workspaces.create(repo=REPO, name="w1", stream="main").name == "w1"
    assert "w1" in [w.name for w in alice.workspaces.list(repo=REPO).workspaces]
    m1 = alice.workspaces.modify(
        repo=REPO, name="w1", message="seed", changes=[put_file("a.txt", "a\n"), put_file("b/c.txt", "c\n")]
    )
    assert alice.workspaces.get(repo=REPO, name="w1").head == m1.head
    assert alice.workspaces.sync(repo=REPO, name="w1").status == "up_to_date"

    p1 = alice.proposals.create(repo=REPO, stream="main", head=m1.head, title="seed")
    assert len(alice.proposals.list(repo=REPO).proposals) == 1
    m2 = alice.workspaces.modify(repo=REPO, name="w1", message="more", changes=[put_file("d.txt", "d\n")])
    assert alice.proposals.update(repo=REPO, id=p1.id, new_head=m2.head).head == m2.head
    assert alice.proposals.get(repo=REPO, id=p1.id).proposal.head == m2.head
    bob.proposals.review(repo=REPO, id=p1.id, decision="approve")

    queued = alice.integration.validate(repo=REPO, proposal=p1.id)
    assert len(queued.jobs) == 1
    first = ci.jobs.claim(repo=REPO, worker="tour").job
    assert first is not None
    ci.jobs.heartbeat(repo=REPO, job_id=first.id, worker="tour")
    ci.jobs.fail(repo=REPO, job_id=first.id, worker="tour", error="flaky", retry_after_ms=1)
    time.sleep(0.02)
    second = ci.jobs.claim(repo=REPO, worker="tour", wait_ms=3000).job
    assert second is not None
    assert second.validation is not None
    tree = ci.jobs.candidate(repo=REPO, proposal=p1.id, expected_tree_id=second.validation.tree_id)
    assert tree.total_entries == 3
    v = {
        "tree_id": second.validation.tree_id,
        "check": second.validation.check,
        "policy_version": second.validation.policy_version,
    }
    assert (
        ci.jobs.complete(repo=REPO, job_id=second.id, worker="tour", attestation=attest(v)).state
        == "succeeded"
    )
    assert ci.jobs.get(repo=REPO, job_id=second.id).state == "succeeded"
    assert len(ci.jobs.list(repo=REPO).jobs) == 1
    assert len(carol.integration.attestations(repo=REPO, tree_id=second.validation.tree_id).attestations) == 1

    assert carol.integration.prepare(repo=REPO, proposal=p1.id).blockers == []
    assert carol.queue.enqueue(repo=REPO, proposal=p1.id).state == "queued"
    assert len(carol.queue.list(repo=REPO, stream="main").entries) == 1
    ran = carol.queue.run(repo=REPO, stream="main")
    assert (ran.processed[0].state, ran.processed[0].outcome) == ("done", "integrated")
    Seed.head = m2.head

    alice.workspaces.create(repo=REPO, name="w2", stream="main")
    m3 = alice.workspaces.modify(repo=REPO, name="w2", changes=[put_file("e.txt", "e\n")])
    p2 = alice.proposals.create(repo=REPO, stream="main", head=m3.head, title="second")
    bob.proposals.review(repo=REPO, id=p2.id, decision="comment", body="hm")
    bob.proposals.review(repo=REPO, id=p2.id, decision="approve")
    prep = carol.integration.prepare(repo=REPO, proposal=p2.id)
    v2 = {"tree_id": prep.candidate_tree, "check": "tests", "policy_version": prep.policy_version}
    assert ci.integration.attest(repo=REPO, attestation=attest(v2)).accepted
    assert carol.integration.integrate(repo=REPO, proposal=p2.id).outcome.status == "integrated"

    alice.workspaces.create(repo=REPO, name="w3", stream="main")
    m4 = alice.workspaces.modify(repo=REPO, name="w3", changes=[put_file("f.txt", "f\n")])
    p3 = alice.proposals.create(repo=REPO, stream="main", head=m4.head, title="third")
    assert alice.proposals.close(repo=REPO, id=p3.id).state == "closed"
    alice.workspaces.delete(repo=REPO, name="w3")


def test_history_trees_streams_and_objects(tour: dict[str, AgentGitClient]) -> None:
    admin, alice = tour["admin"], tour["alice"]
    head = alice.streams.head(repo=REPO, stream="main").head
    assert alice.history.resolve(repo=REPO, rev="main").changeset_id == head
    assert len(alice.history.log(repo=REPO, rev="main").changesets) > 1
    assert alice.history.changeset(repo=REPO, rev="main", include_ops=True).id == head
    assert alice.history.merge_base(repo=REPO, a="main", b=Seed.head).bases == [Seed.head]
    assert alice.history.ancestors(repo=REPO, want=[head], have=[Seed.head]).changeset_ids == [head]

    assert [m.path for m in alice.tree.search(repo=REPO, rev="main", query="c", path_prefix="b").matches] == [
        "b/c.txt"
    ]
    assert alice.tree.get(repo=REPO, rev="main").total_entries == 4
    assert [e.name for e in alice.tree.list(repo=REPO, rev="main", path="b").entries] == ["c.txt"]
    assert alice.tree.resolve(repo=REPO, rev="main", path="a.txt").kind == "file"
    assert [f.path for f in alice.tree.diff(repo=REPO, from_=Seed.head, to="main").files] == ["e.txt"]
    assert alice.tree.file(repo=REPO, path="b/c.txt", rev="main") == b"c\n"

    assert "main" in [s.name for s in alice.streams.list(repo=REPO).streams]
    alice.streams.create(repo=REPO, name="dev")
    cas = admin.streams.cas(
        repo=REPO, stream="dev", expected_version=0, expected_head="", new_head=head, reason="tour"
    )
    assert (cas.name, cas.head, cas.version) == ("dev", head, 1)
    assert len(alice.streams.reflog(repo=REPO, stream="dev").entries) == 1

    ns = alice.repos.describe(repo=REPO).namespace
    built = build_changeset(
        namespace=ns,
        parents=[head],
        author="user:alice",
        timestamp_ms=1_760_000_000_000,
        ops=[PutOp("g.txt", content="g\n")],
    )
    everything = built.objects()
    assert len(alice.objects.missing(repo=REPO, object_ids=[o.id for o in everything]).missing) == 2
    for o in everything:
        alice.objects.put(repo=REPO, object_id=o.id, data=o.envelope)
    assert alice.objects.missing(repo=REPO, object_ids=[o.id for o in everything]).missing == []
    assert alice.objects.get(repo=REPO, object_id=blob_object("g\n").id) == blob_object("g\n").envelope
    assert alice.objects.verify(repo=REPO, object_ids=[built.id]).objects[0].status == "ok"
    pub = alice.objects.publish(repo=REPO, head=built.id, idempotency_key="tour-publish")
    assert pub.state == "published"
    assert alice.operations.get(repo=REPO, idempotency_key="tour-publish").kind == "publish"


def test_line_level_blame(tour: dict[str, AgentGitClient]) -> None:
    blame = tour["alice"].history.blame(repo=REPO, path="b/c.txt", rev="main")
    assert [(ln.line, ln.text, ln.path) for ln in blame.lines] == [(1, "c", "b/c.txt")]
    assert re.fullmatch(r"[0-9a-f]{64}", blame.lines[0].changeset)


def test_agents_tasks_leases_and_provenance(tour: dict[str, AgentGitClient]) -> None:
    alice, bot = tour["alice"], tour["bot"]
    key = generate_agent_key()
    bot.agents.register(repo=REPO, name="tour-bot", public_key=key.public_key_hex)
    assert "tour-bot" in [a.name for a in alice.agents.list(repo=REPO).agents]
    tour["admin"].agents.executor_report(
        repo=REPO, agent="tour-bot", name="command", slots=1, available=1, ttl_secs=90
    )
    task = alice.tasks.create(repo=REPO, title="tour task", scope=["src/**"], assignee="tour-bot")
    alice.tasks.assign(repo=REPO, id=task.id, agent="tour-bot", expected_owner="agent:tour-bot")
    dep = alice.tasks.create(repo=REPO, title="depends")
    alice.tasks.depend(repo=REPO, task=dep.id, depends_on=task.id)
    assert len(alice.tasks.list(repo=REPO).tasks) == 2
    run = bot.agents.start_run(repo=REPO, agent="tour-bot", task_id=task.id)
    assert len(alice.agents.list_runs(repo=REPO, active_only=True).runs) == 1
    assert alice.agents.get_run(repo=REPO, run_id=run.run_id).identity == run.identity
    runner = bot.with_run(run.run_id)
    runner.agents.heartbeat(repo=REPO, run_id=run.run_id)
    lease = runner.leases.acquire(repo=REPO, scope="src/**", exclusive=True, ttl_ms=30_000, task_id=task.id)
    assert len(alice.leases.list(repo=REPO, active_only=True).leases) == 1
    runner.leases.extend(repo=REPO, lease_id=lease.lease.id, ttl_ms=60_000)
    runner.leases.release(repo=REPO, lease_id=lease.lease.id)
    assert isinstance(alice.agents.predict(repo=REPO, stream="main").overlaps, list)

    head = alice.streams.head(repo=REPO, stream="main").head
    record = {
        "changeset": head,
        "agent": run.identity,
        "run_id": run.run_id,
        "model": "m",
        "created_at_ms": int(time.time() * 1000),
    }
    entry = runner.provenance.submit(
        repo=REPO, record=record, signature=sign_provenance(record, key.private_key)
    )
    assert entry.verified
    assert len(alice.provenance.list(repo=REPO, changeset=head).entries) == 1
    runner.tasks.finish(repo=REPO, id=task.id, state="done")
    runner.agents.complete(repo=REPO, run_id=run.run_id, status="completed")
    assert alice.tasks.get(repo=REPO, id=dep.id).ready
    # The operator's call: nothing has been silent for an hour, so a dry run finds nothing to end.
    plan = tour["admin"].agents.reap_runs(repo=REPO, stale_after_ms=3_600_000, dry_run=True)
    assert (plan.dry_run, plan.runs, plan.released_leases, plan.revoked_credentials) == (True, [], [], 0)


def test_events_audit_and_maintenance(tour: dict[str, AgentGitClient]) -> None:
    admin, alice = tour["admin"], tour["alice"]
    assert len(alice.events.replay(repo=REPO, after_seq=0, limit=5).events) == 5
    got = []
    for ev in alice.events.subscribe(repo=REPO, after_seq=0):
        got.append(ev.topic)
        if len(got) == 3:
            break
    assert len(got) == 3
    assert len(admin.admin.audit(repo=REPO, limit=5).entries) == 5
    assert admin.admin.audit_verify(repo=REPO).ok
    head = alice.streams.head(repo=REPO, stream="main").head
    admin.admin.pin(repo=REPO, name="release-1", rev=head, reason="tour")
    assert [p.name for p in admin.admin.pins(repo=REPO).pins] == ["release-1"]
    admin.admin.unpin(repo=REPO, name="release-1")
    assert admin.admin.fsck(repo=REPO).ok
    assert admin.admin.gc(repo=REPO, dry_run=True).dry_run


def test_operator_scheduling_gate(tour: dict[str, AgentGitClient]) -> None:
    admin = tour["admin"]
    initial = admin.tasks.scheduling_get(repo=REPO)
    paused = admin.tasks.scheduling_set(
        repo=REPO, paused=True, expected_version=initial.version, reason="Upgrade"
    )
    assert paused.paused
    resumed = admin.tasks.scheduling_set(repo=REPO, paused=False, expected_version=paused.version)
    assert not resumed.paused


def test_issue_discussion(tour: dict[str, AgentGitClient]) -> None:
    alice = tour["alice"]
    issue = alice.issues.create(repo=REPO, title="Move the project", body="Keep history")
    assert alice.issues.get(repo=REPO, number=issue.number).title == issue.title
    assert any(item.number == issue.number for item in alice.paginate("issue.list", repo=REPO))
    comment = alice.issues.comment(repo=REPO, number=issue.number, body="Ready")
    assert comment.body == "Ready"
    closed = alice.issues.update(
        repo=REPO,
        number=issue.number,
        title=issue.title,
        body=issue.body,
        state="closed",
        expected_version=issue.version,
    )
    assert closed.state == "closed"
    assert len(list(alice.paginate("issue.activity", repo=REPO, number=issue.number))) == 3


def test_git_migration(tour: dict[str, AgentGitClient]) -> None:
    admin = tour["admin"]
    data = (REPO_ROOT / "sdk/fixtures/git-migration.bundle").read_bytes()
    options = CallOptions(idempotency_key="python-migration-tour", timeout_ms=300_000)
    plan = admin.migrations.prepare(name="tour/migrated", data=data, options=options)
    assert admin.migrations.prepare(name="tour/migrated", data=data, options=options).id == plan.id
    assert admin.migrations.status(id=plan.id).state == "ready"
    assert admin.migrations.commit(id=plan.id, accept_lossy=False).state == "imported"
    assert admin.migrations.commit(id=plan.id, accept_lossy=False).state == "imported"
    assert admin.tree.file(repo="tour/migrated", path="hello.txt", rev="main") == b"hello from migration\n"


def test_the_async_client_reaches_the_operations_too(server: RunningServer) -> None:
    import asyncio

    async def scenario() -> set[str]:
        net = AsyncFaultyTransport()
        async with AsyncAgentGitClient(
            server.base_url, server.bootstrap_token, http_client=httpx.AsyncClient(transport=net)
        ) as c:
            plan = await c.migrations.prepare(
                name="tour/migrated-async",
                data=(REPO_ROOT / "sdk/fixtures/git-migration.bundle").read_bytes(),
            )
            assert (await c.migrations.status(id=plan.id)).state == "ready"
            assert (await c.migrations.commit(id=plan.id, accept_lossy=False)).state == "imported"
            await c.session.negotiate()
            await c.session.whoami()
            await c.repos.create(name="tour/async")
            await c.repos.describe(repo="tour/async")
            await c.workspaces.create(repo="tour/async", name="w", stream="main")
            m = await c.workspaces.modify(repo="tour/async", name="w", changes=[put_file("a", "a")])
            p = await c.proposals.create(repo="tour/async", stream="main", head=m.head, title="t")
            await c.integration.integrate(repo="tour/async", proposal=p.id)
            assert await c.tree.file(repo="tour/async", path="a", rev="main") == b"a"
            await c.call("history.log", repo="tour/async", rev="main")
        return {s.method + " " + s.path.split("?")[0] for s in net.seen}

    assert len(asyncio.run(scenario())) >= 8


def test_every_operation_of_the_openapi_document_has_a_method_in_both_clients() -> None:
    spec = json.loads((REPO_ROOT / "docs/protocol/openapi.json").read_text())
    ids = sorted(o["operationId"] for p in spec["paths"].values() for o in p.values())
    assert sorted(i for ns in NAMESPACE_METHODS.values() for i in ns.values()) == ids
    assert sorted(OPERATIONS) == ids
    sync = AgentGitClient("http://127.0.0.1:1")
    asy = AsyncAgentGitClient("http://127.0.0.1:1")
    for ns, methods in NAMESPACE_METHODS.items():
        for name in methods:
            assert callable(getattr(getattr(sync, ns), name)), f"{ns}.{name}"
            assert callable(getattr(getattr(asy, ns), name)), f"async {ns}.{name}"


def test_the_tour_reached_every_operation_and_no_route_was_missing(recorder: Recorder) -> None:
    missing = sorted(set(OPERATIONS) - recorder.reached)
    assert missing == []
    assert recorder.route_missing == []
