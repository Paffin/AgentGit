// #region review-requests
import type { AgentGitClient } from '@agentgit/client';

/** Replace the inspected routing list. A concurrent edit needs a fresh user decision. */
export async function requestReview(client: AgentGitClient, repo: string, id: string, reviewers: string[]) {
  const current = await client.proposals.get({ repo, id });
  return client.proposals.setReviewers({
    repo,
    id,
    reviewers,
    expected_reviewers: current.requested_reviewers ?? [],
  });
}
/** Gate the filter: older servers can ignore unknown query fields and return a broader list. */
export async function* reviewsRequestedFrom(client: AgentGitClient, repo: string, reviewer: string) {
  const hello = await client.negotiate();
  if (!hello.features.includes('proposals.review_requests'))
    throw new Error('This server does not support requested-reviewer filtering.');
  yield* client.paginate('proposal.list', { repo, requested_reviewer: reviewer });
}
// #endregion review-requests
