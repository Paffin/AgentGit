"""Required checks: a trusted worker claims a validation job, signs an attestation for the exact
candidate tree and completes the job; only then can the change land."""

from __future__ import annotations

import json
import os
import time
from typing import Any

import pytest

from agentgit import (
    AgentGitClient,
    FailedPreconditionError,
    InvalidArgumentError,
    PermissionDeniedError,
    StaleCandidateError,
    generate_agent_key,
    put_file,
    sign_attestation,
)
from agentgit.models import Job
from tests.harness import RunningServer

REPO = "acme/ci"
CI_KEY = generate_agent_key()


@pytest.fixture(scope="module")
def team(server: RunningServer) -> dict[str, AgentGitClient]:
    admin = server.admin()
    admin.repos.create(name=REPO)
    admin.admin.policy_set(
        repo=REPO,
        stream="main",
        document=json.dumps(
            {
                "trusted_issuers": [{"identity": "service:ci", "public_key": CI_KEY.public_key_hex}],
                "required_validations": [
                    {
                        "check": "tests",
                        "issuers": ["service:ci"],
                        "run": {"argv": ["cargo", "test"], "timeout_secs": 600},
                    }
                ],
            }
        ),
    )
    return {
        "admin": admin,
        "alice": server.actor("user:alice", REPO, "writer"),
        "carol": server.actor("user:carol", REPO, "integrator"),
        "ci": server.actor("service:ci", REPO, "runner"),
    }


def propose(alice: AgentGitClient, name: str, file: str) -> tuple[str, str]:
    alice.workspaces.create(repo=REPO, name=name, stream="main")
    ws = alice.workspaces.modify(repo=REPO, name=name, changes=[put_file(file, f"{name}\n")])
    p = alice.proposals.create(repo=REPO, stream="main", head=ws.head, title=name)
    return p.id, ws.head


def attestation(job_or_tree: Job | dict[str, Any], result: str = "pass") -> dict[str, Any]:
    if isinstance(job_or_tree, Job):
        assert job_or_tree.validation is not None
        v: dict[str, Any] = {
            "tree_id": job_or_tree.validation.tree_id,
            "check": job_or_tree.validation.check,
            "policy_version": job_or_tree.validation.policy_version,
        }
    else:
        v = job_or_tree
    return sign_attestation(
        {
            "id": f"att_{os.urandom(10).hex()}",
            **v,
            "result": result,
            "environment_digest": "sha256:sdk-test",
            "issuer": "service:ci",
            "issued_at_ms": int(time.time() * 1000),
        },
        CI_KEY,
    )


def test_a_job_is_queued_claimed_attested_and_completed_then_the_change_lands(
    team: dict[str, AgentGitClient],
) -> None:
    alice, carol, ci = team["alice"], team["carol"], team["ci"]
    pid, _ = propose(alice, "feature-1", "one.txt")
    blocked = carol.integration.integrate(repo=REPO, proposal=pid)
    assert blocked.outcome.status == "blocked"
    assert [b.code for b in blocked.outcome.blockers] == ["missing_validation"]

    prep = alice.integration.prepare(repo=REPO, proposal=pid, enqueue_validations=True)
    assert [(v.check, v.state) for v in prep.validations] == [("tests", "pending")]
    assert len(prep.jobs) == 1
    again = alice.integration.validate(repo=REPO, proposal=pid)  # does not queue a second job
    assert [j.id for j in again.jobs] == [j.id for j in prep.jobs]

    with pytest.raises(PermissionDeniedError):  # nothing for a caller who is not a runner
        alice.jobs.claim(repo=REPO, worker="nope")

    claimed = ci.jobs.claim(repo=REPO, worker="worker-1", kinds=["validate"], lease_ms=60_000)
    job = claimed.job
    assert job is not None
    assert (job.state, job.kind, job.attempts) == ("claimed", "validate", 1)
    assert job.claimed_by == "service:ci|worker-1"
    assert job.validation is not None
    assert (job.validation.proposal, job.validation.check) == (pid, "tests")
    assert job.validation.run is not None
    assert job.validation.run.argv == ["cargo", "test"]

    # The worker checks out exactly the tree it was asked to check ...
    files = [
        e.path
        for e in ci.paginate(
            "jobs.candidate", repo=REPO, proposal=pid, expected_tree_id=job.validation.tree_id
        )
    ]
    assert files == ["one.txt"]
    # ... and is told if the candidate changed in the meantime.
    with pytest.raises(StaleCandidateError):
        ci.jobs.candidate(repo=REPO, proposal=pid, expected_tree_id="0" * 64)

    beat = ci.jobs.heartbeat(repo=REPO, job_id=job.id, worker="worker-1", lease_ms=120_000)
    assert beat.claimed_until_ms > job.claimed_until_ms
    assert ci.jobs.get(repo=REPO, job_id=job.id).state == "claimed"

    # Another worker cannot finish this job; an attestation for another check is refused.
    with pytest.raises(FailedPreconditionError):
        ci.jobs.complete(repo=REPO, job_id=job.id, worker="worker-2", attestation=attestation(job))
    with pytest.raises(InvalidArgumentError):
        ci.jobs.complete(
            repo=REPO, job_id=job.id, worker="worker-1", attestation={**attestation(job), "check": "lint"}
        )

    att = attestation(job)
    done = ci.jobs.complete(
        repo=REPO, job_id=job.id, worker="worker-1", attestation=att, result={"exit_code": 0}
    )
    assert done.state == "succeeded"
    assert done.result["attestation"] == att["id"]
    assert (done.result["outcome"], done.result["exit_code"]) == ("pass", 0)

    listed = carol.integration.attestations(repo=REPO, tree_id=job.validation.tree_id)
    assert [a.id for a in listed.attestations] == [att["id"]]
    assert carol.integration.integrate(repo=REPO, proposal=pid).outcome.status == "integrated"
    assert job.id in [j.id for j in alice.jobs.list(repo=REPO, state="succeeded").jobs]


def test_a_failing_check_denies_the_change_and_a_failed_job_can_be_retried(
    team: dict[str, AgentGitClient],
) -> None:
    alice, carol, ci = team["alice"], team["carol"], team["ci"]
    pid, _ = propose(alice, "feature-2", "two.txt")
    jobs = alice.integration.validate(repo=REPO, proposal=pid).jobs
    claimed = ci.jobs.claim(repo=REPO, worker="worker-1").job
    assert claimed is not None
    assert claimed.id == jobs[0].id
    failed = ci.jobs.fail(
        repo=REPO, job_id=claimed.id, worker="worker-1", error="runner crashed", retry_after_ms=1
    )
    assert failed.state == "queued"
    time.sleep(0.02)
    second = ci.jobs.claim(repo=REPO, worker="worker-1", wait_ms=2000).job
    assert second is not None
    assert (second.id, second.attempts) == (claimed.id, 2)

    ci.jobs.complete(repo=REPO, job_id=second.id, worker="worker-1", attestation=attestation(second, "fail"))
    denied = carol.integration.integrate(repo=REPO, proposal=pid)
    assert denied.outcome.status == "denied"
    assert "failed_validation" in [b.code for b in denied.outcome.blockers]


def test_claiming_with_nothing_queued_answers_none_and_a_long_poll_returns_when_work_arrives(
    team: dict[str, AgentGitClient],
) -> None:
    import threading

    alice, ci = team["alice"], team["ci"]
    assert ci.jobs.claim(repo=REPO, worker="idle").job is None
    result: list[Any] = []
    started = time.monotonic()
    t = threading.Thread(
        target=lambda: result.append(ci.jobs.claim(repo=REPO, worker="poller", wait_ms=10_000).job)
    )
    t.start()
    time.sleep(0.2)
    pid, _ = propose(alice, "feature-3", "three.txt")
    alice.integration.validate(repo=REPO, proposal=pid)
    t.join(10)
    job = result[0]
    assert job is not None
    assert job.validation.proposal == pid
    assert time.monotonic() - started < 8
    ci.jobs.complete(repo=REPO, job_id=job.id, worker="poller", stale=True)
    assert ci.jobs.get(repo=REPO, job_id=job.id).state == "succeeded"


def test_a_trusted_attestation_can_be_submitted_directly_and_an_untrusted_one_counts_for_nothing(
    team: dict[str, AgentGitClient],
) -> None:
    alice, carol, ci = team["alice"], team["carol"], team["ci"]
    pid, _ = propose(alice, "feature-4", "four.txt")
    prep = carol.integration.prepare(repo=REPO, proposal=pid)
    v = {"tree_id": prep.candidate_tree, "check": "tests", "policy_version": prep.policy_version}
    assert ci.integration.attest(repo=REPO, attestation=attestation(v)).accepted
    assert carol.integration.integrate(repo=REPO, proposal=pid).outcome.status == "integrated"

    # A signature that does not match the content is refused outright ...
    with pytest.raises(InvalidArgumentError):
        ci.integration.attest(repo=REPO, attestation={**attestation(v), "check": "lint"})
    # ... and a well-formed one from a key the policy does not trust is stored but counts for nothing.
    qid, _ = propose(alice, "feature-5", "five.txt")
    prep2 = carol.integration.prepare(repo=REPO, proposal=qid)
    stranger = sign_attestation(
        {
            "id": "att_" + "0" * 20,
            "tree_id": prep2.candidate_tree,
            "check": "tests",
            "result": "pass",
            "policy_version": prep2.policy_version,
            "environment_digest": "sha256:sdk-test",
            "issuer": "service:ci",
            "issued_at_ms": int(time.time() * 1000),
        },
        generate_agent_key(),
    )
    ci.integration.attest(repo=REPO, attestation=stranger)
    still = carol.integration.integrate(repo=REPO, proposal=qid)
    assert still.outcome.status == "blocked"
    assert [b.code for b in still.outcome.blockers] == ["missing_validation"]
