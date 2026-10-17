"""sdk/api-policy.json lists message fields that the protocol makes optional but that the SDK types as
always present, because ag-server fills them in. This fails if the server ever answers null for one."""

from __future__ import annotations

import json

from agentgit import put_file
from tests.harness import REPO_ROOT, RunningServer

POLICY = json.loads((REPO_ROOT / "sdk/api-policy.json").read_text())


def test_every_field_typed_as_present_is_a_non_null_object_on_a_live_server(server: RunningServer) -> None:
    seen: dict[str, object] = {}
    admin = server.admin()
    repo = "acme/contract"
    seen["Repo.quota"] = admin.repos.create(name=repo).quota
    alice = server.actor("user:alice", repo, "writer")
    carol = server.actor("user:carol", repo, "integrator")
    seen["Principal.scope"] = alice.session.whoami().scope
    seen["NegotiateResponse.limits"] = admin.session.negotiate().limits
    minted = admin.credentials.mint(subject="user:x", ttl_secs=60)
    seen["MintCredentialResponse.credential"] = minted.credential
    seen["Credential.scope"] = minted.credential.scope

    alice.workspaces.create(repo=repo, name="w", stream="main")
    ws = alice.workspaces.modify(repo=repo, name="w", changes=[put_file("a", "a")])
    p = alice.proposals.create(repo=repo, stream="main", head=ws.head, title="t")
    seen["ProposalDetail.proposal"] = alice.proposals.get(repo=repo, id=p.id).proposal
    seen["IntegrationResult.outcome"] = carol.integration.integrate(repo=repo, proposal=p.id).outcome
    bot = server.actor("agent:contract-bot", repo, "writer")
    seen["AcquireLeaseResponse.lease"] = bot.leases.acquire(repo=repo, scope="x/**", ttl_ms=10_000).lease

    for field in POLICY["alwaysPresent"]["fields"]:
        assert field in seen, f"{field} is not checked by this test"
        assert seen[field] is not None, field
        assert not isinstance(seen[field], (str, int, float, bool, list)), field
