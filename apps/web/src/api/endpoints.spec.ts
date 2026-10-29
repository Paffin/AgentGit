// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import { ApiClient } from './client'
import { createApi } from './endpoints'

const invoke = (api: ReturnType<typeof createApi>, operation: string) => {
  const options = { token: 'initial-session', quietUnauthenticated: true }
  if (operation === 'revoke')
    return api.admin.revokeRole(
      { subject: 'user:alice', repo: 'acme/new', role: 'reader' },
      options,
    )
  if (operation === 'grant')
    return api.admin.grantRole({ subject: 'user:alice', repo: 'acme/new', role: 'writer' }, options)
  if (operation === 'workspace-create')
    return api.workspaces.create('acme/new', { name: 'edit', stream: 'main' }, options)
  if (operation === 'workspace-modify')
    return api.workspaces.modify(
      'acme/new',
      'edit',
      { changes: [], expected_head: 'base' },
      options,
    )
  if (operation === 'workspace-remove') return api.workspaces.remove('acme/new', 'edit', options)
  if (operation === 'review')
    return api.proposals.review(
      'acme/new',
      'proposal-one',
      { decision: 'comment', expected_head: 'head' },
      options,
    )

  if (operation === 'reviewers')
    return api.proposals.setReviewers('acme/new', 'proposal-one', ['user:bob'], [], options)

  if (operation === 'proposal-update')
    return api.proposals.update('acme/new', 'proposal-one', 'new-head', 'seen-head', options)

  if (operation === 'task-dependency')
    return api.agents.addDependency('acme/new', 'task-one', 'task-two', options)
  if (operation === 'task-attempt-limit')
    return api.agents.setTaskAttemptLimit('acme/new', 'task-one', 3, 2, options)
  if (operation === 'task-finish')
    return api.agents.finishTask('acme/new', 'task-one', 'done', options)
  if (operation === 'proposal-create')
    return api.proposals.create(
      'acme/new',
      { stream: 'main', head: 'head', title: 'Edit' },
      options,
    )
  return operation === 'create'
    ? api.repos.create({ name: 'acme/new' }, options)
    : api.admin.setPolicy('acme/new', 'main', '{}', options)
}

describe('repository setup request ownership', () => {
  it.each([
    'create',
    'policy',
    'grant',
    'revoke',
    'workspace-create',
    'workspace-modify',
    'workspace-remove',
    'proposal-create',
    'proposal-update',
    'reviewers',
    'review',

    'task-dependency',
    'task-finish',
    'task-attempt-limit',
  ])('keeps the initial session during a %s retry', async (operation) => {
    let token = 'initial-session'
    const credentials: (string | null)[] = []
    const fetchMock = vi.fn<typeof fetch>(async (_input, init) => {
      credentials.push(new Headers(init?.headers).get('authorization'))
      if (credentials.length === 1) {
        token = 'new-session'
        return new Response(JSON.stringify({ error: { code: 'unavailable', message: 'Retry' } }), {
          status: 503,
          headers: { 'content-type': 'application/json' },
        })
      }
      return new Response('{}', { headers: { 'content-type': 'application/json' } })
    })
    const client = new ApiClient({
      getToken: () => token,
      fetch: fetchMock,
      maxRetries: 1,
      sleep: async () => {},
    })
    await invoke(createApi(client), operation)
    expect(credentials).toEqual(['Bearer initial-session', 'Bearer initial-session'])
  })

  it.each([
    'create',
    'policy',
    'grant',
    'revoke',
    'workspace-create',
    'workspace-modify',
    'workspace-remove',
    'proposal-create',
    'proposal-update',
    'reviewers',
    'review',

    'task-dependency',
    'task-finish',
    'task-attempt-limit',
  ])('does not reject a new session when an old %s request receives 401', async (operation) => {
    const rejected = vi.fn()
    const client = new ApiClient({
      getToken: () => 'new-session',
      onUnauthenticated: rejected,
      fetch: vi.fn<typeof fetch>().mockResolvedValue(
        new Response(JSON.stringify({ error: { code: 'unauthenticated', message: 'Expired' } }), {
          status: 401,
          headers: { 'content-type': 'application/json' },
        }),
      ),
    })
    await expect(invoke(createApi(client), operation)).rejects.toMatchObject({ status: 401 })
    expect(rejected).not.toHaveBeenCalled()
  })
})

it('sends the inspected proposal head with a replacement', async () => {
  let sent: unknown
  const client = new ApiClient({
    getToken: () => '',
    fetch: async (_input, init) => {
      sent = JSON.parse(String(init?.body))
      return new Response('{}', { headers: { 'content-type': 'application/json' } })
    },
  })
  await createApi(client).proposals.update('acme/repo', 'proposal', 'next', 'inspected')
  expect(sent).toEqual({ new_head: 'next', expected_head: 'inspected' })
})

it('patches an attempt limit with its inspected value and an encoded task id', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response('{}', { headers: { 'content-type': 'application/json' } }))
  const client = new ApiClient({ getToken: () => 'session', fetch: request })
  await createApi(client).agents.setTaskAttemptLimit('acme/repo', 'task/one', 0, 2)
  const [url, options] = request.mock.calls[0]!
  expect(String(url)).toContain('/v1/repos/acme/repo/tasks/task%2Fone/attempt-limit')
  expect(options?.method).toBe('PATCH')
  expect(JSON.parse(String(options?.body))).toEqual({ max_attempts: 0, expected_max_attempts: 2 })
})

it('sends requested reviewers and the inspected set to the encoded proposal path', async () => {
  const request = vi.fn<typeof fetch>().mockResolvedValue(
    new Response('{"reviewers":["user:bob"]}', {
      headers: { 'content-type': 'application/json' },
    }),
  )
  const client = new ApiClient({ getToken: () => 'session', fetch: request })
  await createApi(client).proposals.setReviewers('acme/repo', 'p/one', ['user:bob'], ['user:carol'])
  const [url, options] = request.mock.calls[0]!
  expect(String(url)).toContain('/v1/repos/acme/repo/proposals/p%2Fone/reviewers')
  expect(options?.method).toBe('PUT')
  expect(JSON.parse(String(options?.body))).toEqual({
    reviewers: ['user:bob'],
    expected_reviewers: ['user:carol'],
  })
})

it('pins virtual sync to the inspected branch while retaining the initiating session', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response('{}', { headers: { 'content-type': 'application/json' } }))
  const client = new ApiClient({ getToken: () => 'new-session', fetch: request })
  await createApi(client).workspaces.sync('acme/repo', 'w/one', [], {
    token: 'original-session',
    expectedStreamHead: 'inspected',
  })
  const [url, options] = request.mock.calls[0]!
  expect(String(url)).toContain('/workspaces/w%2Fone/sync')
  expect(JSON.parse(String(options?.body))).toEqual({ expected_stream_head: 'inspected' })
  expect(new Headers(options?.headers).get('authorization')).toBe('Bearer original-session')
})

it('keeps private draft content in the save body and captures the initiating session', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response('{}', { headers: { 'content-type': 'application/json' } }))
  const client = new ApiClient({ getToken: () => 'new-session', fetch: request })
  await createApi(client).proposals.saveReviewDraft(
    'acme/repo',
    'p/one',
    { head: 'seen', decision: 'comment', body: 'private', expected_version: 0 },
    { token: 'original', quietUnauthenticated: true },
  )
  const [url, options] = request.mock.calls[0]!
  expect(String(url)).toContain('/proposals/p%2Fone/review-draft')
  expect(String(url)).not.toContain('private')
  expect(options?.method).toBe('PUT')
  expect(new Headers(options?.headers).get('authorization')).toBe('Bearer original')
  expect(JSON.parse(String(options?.body))).toEqual({
    head: 'seen',
    decision: 'comment',
    body: 'private',
    expected_version: 0,
  })
})

it('sends the clear-draft version in a DELETE query, including zero', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response('{}', { headers: { 'content-type': 'application/json' } }))
  const client = new ApiClient({ getToken: () => 'session', fetch: request })
  await createApi(client).proposals.clearReviewDraft('acme/repo', 'p/one', 0)
  const [url, options] = request.mock.calls[0]!
  expect(String(url)).toContain('/proposals/p%2Fone/review-draft?expected_version=0')
  expect(options?.method).toBe('DELETE')
  expect(options?.body).toBeUndefined()
})

it('keeps account comment drafts distinct from reviews and sends private text only in the body', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockImplementation(
      async () => new Response('{}', { headers: { 'content-type': 'application/json' } }),
    )
  const client = new ApiClient({ getToken: () => 'current-session', fetch: request })
  const proposals = createApi(client).proposals
  await proposals.commentDraft('acme/repo', 'p/one', { token: 'captured-session' })
  await proposals.saveCommentDraft(
    'acme/repo',
    'p/one',
    { head: 'inspected', body: 'unsubmitted notes', expected_version: 7 },
    { token: 'captured-session' },
  )
  expect(request.mock.calls).toHaveLength(2)
  for (const [url, options] of request.mock.calls) {
    expect(String(url)).toContain('/proposals/p%2Fone/comment-draft')
    expect(String(url)).not.toContain('unsubmitted')
    expect(new Headers(options?.headers).get('authorization')).toBe('Bearer captured-session')
  }
  const [, saved] = request.mock.calls[1]!
  expect(saved?.method).toBe('PUT')
  expect(JSON.parse(String(saved?.body))).toEqual({
    head: 'inspected',
    body: 'unsubmitted notes',
    expected_version: 7,
  })
})

it('clears a private comment draft using its inspected version query', async () => {
  const request = vi
    .fn<typeof fetch>()
    .mockResolvedValue(new Response('{}', { headers: { 'content-type': 'application/json' } }))
  const client = new ApiClient({ getToken: () => 'session', fetch: request })
  await createApi(client).proposals.clearCommentDraft('acme/repo', 'p/one', 0)
  const [url, options] = request.mock.calls[0]!
  expect(String(url)).toContain('/proposals/p%2Fone/comment-draft?expected_version=0')
  expect(options?.method).toBe('DELETE')
  expect(options?.body).toBeUndefined()
})
