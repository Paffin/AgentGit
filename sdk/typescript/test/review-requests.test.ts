import { describe, expect, it } from 'vitest';
import { requestReview, reviewsRequestedFrom } from '../examples/review-requests.js';
import { AgentGitClient, type ProposalDetail, type Res } from '../src/index.js';

const baseUrl = 'http://review.test';

function urlOf(input: Parameters<typeof fetch>[0]): string {
  return typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
}
function bodyOf(init: RequestInit | undefined): string {
  if (typeof init?.body !== 'string') throw new Error('Expected a JSON request body');
  return init.body;
}

describe('requested reviewers', () => {
  it('sends the inspected reviewer set without path fields in the body', async () => {
    const requests: { url: string; method: string; body: unknown }[] = [];
    const client = new AgentGitClient({
      baseUrl,
      token: 'test',
      fetch: async (input, init) => {
        requests.push({ url: urlOf(input), method: init?.method ?? 'GET', body: JSON.parse(bodyOf(init)) });
        return Response.json({ reviewers: ['user:carol'] });
      },
    });
    const result = await client.proposals.setReviewers({
      repo: 'team/app',
      id: 'proposal #1',
      reviewers: ['user:carol'],
      expected_reviewers: ['user:bob'],
    });
    expect(result.reviewers).toEqual(['user:carol']);
    expect(requests).toEqual([
      {
        url: `${baseUrl}/v1/repos/team/app/proposals/proposal%20%231/reviewers`,
        method: 'PUT',
        body: { reviewers: ['user:carol'], expected_reviewers: ['user:bob'] },
      },
    ]);
  });

  it('retries a lost response with the same inspected set', async () => {
    const bodies: string[] = [];
    const client = new AgentGitClient({
      baseUrl,
      token: 'test',
      sleep: async () => {},
      fetch: async (_input, init) => {
        bodies.push(bodyOf(init));
        if (bodies.length === 1) throw new TypeError('response lost after commit');
        return Response.json({ reviewers: ['user:bob'] });
      },
    });
    await expect(
      client.proposals.setReviewers({
        repo: 'team/app',
        id: 'p1',
        reviewers: ['user:bob'],
        expected_reviewers: [],
      }),
    ).resolves.toEqual({ reviewers: ['user:bob'] });
    expect(bodies).toHaveLength(2);
    expect(bodies[1]).toBe(bodies[0]);
  });

  it('retains the requested-reviewer filter across paginated requests', async () => {
    const urls: URL[] = [];
    const client = new AgentGitClient({
      baseUrl,
      token: 'test',
      fetch: async (input) => {
        urls.push(new URL(urlOf(input)));
        return Response.json({
          proposals: [{ id: `p${urls.length}` }],
          next_page_token: urls.length === 1 ? 'next' : '',
        });
      },
    });
    const ids: string[] = [];
    for await (const proposal of client.paginate('proposal.list', {
      repo: 'team/app',
      requested_reviewer: 'user:bob',
      page_size: 1,
    }))
      ids.push(proposal.id);
    expect(ids).toEqual(['p1', 'p2']);
    expect(urls.map((url) => url.searchParams.get('requested_reviewer'))).toEqual(['user:bob', 'user:bob']);
    expect(urls[1]!.searchParams.get('page_token')).toBe('next');
  });
  it('still reads proposal details from servers that omit reviewer requests', async () => {
    const legacyModel: Pick<ProposalDetail, 'requested_reviewers'> = {};
    const legacyResponse: Pick<Res<'proposal.get'>, 'requested_reviewers'> = {};
    expect(legacyModel.requested_reviewers).toBeUndefined();
    expect(legacyResponse.requested_reviewers).toBeUndefined();
    const client = new AgentGitClient({
      baseUrl,
      token: 'test',
      fetch: async () => Response.json({ proposal: { id: 'p1' }, reviews: [] }),
    });
    const detail = await client.proposals.get({ repo: 'team/app', id: 'p1' });
    expect(detail.proposal.id).toBe('p1');
    expect(detail.requested_reviewers).toBeUndefined();
  });
  it('keeps the inspected set when a concurrent edit requires a new decision', async () => {
    const requests: { method: string; body: unknown }[] = [];
    const client = new AgentGitClient({
      baseUrl,
      token: 'test',
      fetch: async (_input, init) => {
        const method = init?.method ?? 'GET';
        requests.push({ method, body: init?.body ? JSON.parse(bodyOf(init)) : undefined });
        return method === 'GET'
          ? Response.json({ proposal: { id: 'p1' }, reviews: [], requested_reviewers: ['user:bob'] })
          : Response.json(
              { error: { code: 'failed_precondition', message: 'reviewers changed', retryable: false } },
              { status: 412 },
            );
      },
    });
    await expect(requestReview(client, 'team/app', 'p1', ['user:carol'])).rejects.toMatchObject({ status: 412 });
    expect(requests).toEqual([
      { method: 'GET', body: undefined },
      { method: 'PUT', body: { reviewers: ['user:carol'], expected_reviewers: ['user:bob'] } },
    ]);
  });
  it('does not issue a reviewer-filtered listing against an unsupported server', async () => {
    const urls: string[] = [];
    const client = new AgentGitClient({
      baseUrl,
      token: 'test',
      fetch: async (input) => {
        urls.push(urlOf(input));
        return Response.json({ protocol_version: 'agwp.v1', features: [] });
      },
    });
    const collect = async () => {
      for await (const proposal of reviewsRequestedFrom(client, 'team/app', 'user:bob'))
        throw new Error(`Unexpected result ${proposal.id} from unsupported server`);
    };
    await expect(collect()).rejects.toThrow('does not support requested-reviewer filtering');
    expect(urls).toHaveLength(1);
    expect(new URL(urls[0]!).pathname).toBe('/v1/capabilities');
  });
});
