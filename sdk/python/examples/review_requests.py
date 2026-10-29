# region review-requests
from collections.abc import Iterator

from agentgit import AgentGitClient, models


def request_review(
    client: AgentGitClient, repo: str, id: str, reviewers: list[str]
) -> models.RequestedReviewers:
    """Replace the inspected routing list; let the caller resolve a concurrent edit."""
    current = client.proposals.get(repo=repo, id=id)
    return client.proposals.set_reviewers(
        repo=repo,
        id=id,
        reviewers=reviewers,
        expected_reviewers=current.requested_reviewers,
    )


def reviews_requested_from(client: AgentGitClient, repo: str, reviewer: str) -> Iterator[models.Proposal]:
    """Check support before filtering: old servers may ignore unknown query fields."""
    hello = client.negotiate()
    if "proposals.review_requests" not in hello.features:
        raise ValueError("This server does not support requested-reviewer filtering.")
    yield from client.paginate("proposal.list", repo=repo, requested_reviewer=reviewer)
    # endregion review-requests
