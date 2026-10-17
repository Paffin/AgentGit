import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { AgentGitClient, AgentGitError, isAgentGitError } from '../src/index.js';
import { startServer, type RunningServer } from './harness/server.js';

let server: RunningServer;
beforeAll(async () => {
  server = await startServer();
});
afterAll(async () => {
  await server?.stop();
});

describe('negotiation and identity', () => {
  it('negotiates without credentials and checks the protocol version', async () => {
    const hello = await server.anonymous().negotiate({ clientName: 'sdk-test', clientVersion: '0.1.0' });
    expect(hello.protocol_version).toBe('agwp.v1');
    expect(hello.features).toContain('events.replay');
    expect(hello.limits?.max_page_size).toBeGreaterThan(0);
    expect(hello.auth_methods).toContain('bearer');
  });

  it('refuses a server that speaks another protocol version', async () => {
    const lying: typeof fetch = async () =>
      new Response(JSON.stringify({ protocol_version: 'agwp.v9', features: [] }), {
        headers: { 'content-type': 'application/json' },
      });
    const client = new AgentGitClient({ baseUrl: server.baseUrl, fetch: lying });
    await expect(client.negotiate()).rejects.toMatchObject({ code: 'failed_precondition' });
  });

  it('whoami reports the identity behind the credential', async () => {
    const me = await server.admin().session.whoami();
    expect(me.method).toBe('bootstrap');
    expect(me.roles).toContain('*:admin');
    const alice = await server.actor('user:alice', 'acme/none', 'reader');
    const who = await alice.session.whoami();
    expect(who.identity).toBe('user:alice');
    expect(who.method).toBe('token');
    expect(who.expires_at_ms).toBeGreaterThan(Date.now());
  });
});

describe('errors', () => {
  it('maps the error envelope onto AgentGitError', async () => {
    const bad = server.anonymous({ token: 'agt_0000000000000000_nope', retry: false });
    const err = await bad.session.whoami().catch((e: unknown) => e);
    expect(isAgentGitError(err)).toBe(true);
    const e = err as AgentGitError;
    expect(e).toBeInstanceOf(Error);
    expect(e.code).toBe('unauthenticated');
    expect(e.status).toBe(401);
    expect(e.retryable).toBe(false);
    expect(e.requestId).toMatch(/^sdk-/); // the id the client sent comes back
    expect(e.operation).toBe('session.whoami');
    expect(e.is('unauthenticated', 'permission_denied')).toBe(true);
  });

  it('reports not_found, already_exists and invalid_argument precisely', async () => {
    const admin = server.admin({ retry: false });
    await admin.repos.create({ name: 'acme/errs' });
    await expect(admin.repos.create({ name: 'acme/errs' })).rejects.toMatchObject({
      code: 'already_exists',
      status: 409,
    });
    await expect(admin.repos.describe({ repo: 'acme/missing' })).rejects.toMatchObject({
      code: 'not_found',
      status: 404,
    });
    await expect(admin.repos.create({ name: 'Not A Valid Name!' })).rejects.toMatchObject({ code: 'invalid_argument' });
  });

  it('hides repositories from callers without a role', async () => {
    await server.admin().repos.create({ name: 'acme/secret' });
    const mallory = await server.actor('user:mallory', 'other/*', 'writer', { retry: false });
    await expect(mallory.repos.describe({ repo: 'acme/secret' })).rejects.toMatchObject({ code: 'not_found' });
    await expect(mallory.repos.create({ name: 'acme/mine' })).rejects.toMatchObject({ code: 'permission_denied' });
  });

  it("a wrong method or an unknown route answers in the standard envelope, with the caller's request id", async () => {
    const auth = { authorization: `Bearer ${server.bootstrapToken}` };
    const wrongMethod = await fetch(`${server.baseUrl}/v1/repos/acme/errs`, {
      method: 'DELETE',
      headers: { ...auth, 'x-request-id': 'req-405' },
    });
    expect(wrongMethod.status).toBe(405);
    expect(wrongMethod.headers.get('allow')).toMatch(/GET/);
    expect(((await wrongMethod.json()) as { error: { code: string; request_id: string } }).error).toMatchObject({
      code: 'invalid_argument',
      request_id: 'req-405',
    });
    const unknown = await fetch(`${server.baseUrl}/v1/nope`, { headers: { ...auth, 'x-request-id': 'req-404' } });
    expect(unknown.status).toBe(404);
    expect(((await unknown.json()) as { error: { code: string; request_id: string } }).error).toMatchObject({
      code: 'not_found',
      request_id: 'req-404',
    });
    // ... which the SDK turns into the usual typed error
    const client = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      retry: false,
      fetch: (input, init) => fetch(input, { ...init, method: 'DELETE' }),
    });
    const err = (await client.repos
      .describe({ repo: 'acme/errs' }, { requestId: 'req-sdk' })
      .catch((e: unknown) => e)) as AgentGitError;
    expect(err).toBeInstanceOf(AgentGitError);
    expect(err).toMatchObject({ status: 405, code: 'invalid_argument', requestId: 'req-sdk', retryable: false });
  });

  it('turns an answer that is not the JSON envelope (a proxy, say) into a typed error', async () => {
    const empty405: typeof fetch = async () => new Response(null, { status: 405 });
    const client = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      retry: false,
      fetch: empty405,
    });
    const err = (await client.repos.describe({ repo: 'acme/errs' }).catch((e: unknown) => e)) as AgentGitError;
    expect(err).toBeInstanceOf(AgentGitError);
    expect(err.status).toBe(405);
    expect(err.code).toBe('http_error');
  });

  it('fails fast, before sending anything, on a malformed repo or a missing path field', async () => {
    const admin = server.admin();
    await expect(admin.repos.describe({ repo: 'no-slash' })).rejects.toThrow(/owner\/name/);
    // @ts-expect-error - id is required for proposals.get
    await expect(admin.proposals.get({ repo: 'acme/errs' })).rejects.toThrow(/missing required field "id"/);
  });
});

describe('requests', () => {
  it('sends the correlation id, deadline and run headers the protocol defines', async () => {
    const seen: Headers[] = [];
    const spy: typeof fetch = async (input, init) => {
      seen.push(new Headers(init?.headers));
      return fetch(input, init);
    };
    const client = new AgentGitClient({
      baseUrl: server.baseUrl,
      token: server.bootstrapToken,
      fetch: spy,
      requestTimeoutMs: 12_345,
      headers: { traceparent: '00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01' },
    });
    await client.repos.describe({ repo: 'acme/errs' }, { requestId: 'my-correlation-id' });
    const h = seen[0]!;
    expect(h.get('authorization')).toBe(`Bearer ${server.bootstrapToken}`);
    expect(h.get('x-request-id')).toBe('my-correlation-id');
    expect(h.get('request-timeout')).toBe('12345');
    expect(h.get('traceparent')).toMatch(/^00-/);
    expect(h.get('idempotency-key')).toBeNull(); // reads carry none
    // The public capabilities route is called without credentials.
    seen.length = 0;
    await client.session.negotiate();
    expect(seen[0]!.get('authorization')).toBeNull();
  });
});
