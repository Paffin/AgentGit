// A `fetch` wrapper around the real network that records every request and can inject faults.

export interface Seen {
  method: string;
  url: string;
  path: string;
  headers: Headers;
  body: string | undefined;
}

export type Fault =
  /** Forward to the server normally. */
  | 'pass'
  /** Forward to the server, then lose the response (the server did process the request). */
  | 'drop-response'
  /** Do not reach the server at all; fail as a refused connection does. */
  | 'refuse'
  /** Answer from here, without reaching the server. */
  | { status: number; headers?: Record<string, string>; body?: string };

export class FaultyNetwork {
  readonly seen: Seen[] = [];
  /** Decides the fate of each request; `n` counts the matching requests so far (1-based). */
  rule: (req: Seen, n: number) => Fault = () => 'pass';
  /** Only requests for which this returns true are counted and given to `rule`. */
  match: (req: Seen) => boolean = () => true;
  #n = 0;

  /** Forgets the requests seen and restarts the count `rule` receives. */
  reset(): void {
    this.seen.length = 0;
    this.#n = 0;
  }

  readonly fetch: typeof fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
    const u = new URL(url);
    const req: Seen = {
      method: init?.method ?? 'GET',
      url,
      path: u.pathname + u.search,
      headers: new Headers(init?.headers),
      body: typeof init?.body === 'string' ? init.body : undefined,
    };
    this.seen.push(req);
    const fault = this.match(req) ? this.rule(req, ++this.#n) : 'pass';
    if (fault === 'refuse')
      throw new TypeError('fetch failed', { cause: new Error('connect ECONNREFUSED (injected)') });
    if (typeof fault === 'object') {
      return new Response(fault.body ?? '', { status: fault.status, headers: fault.headers ?? {} });
    }
    const response = await fetch(input, init);
    if (fault === 'drop-response') {
      await response.arrayBuffer().catch(() => undefined);
      throw new TypeError('fetch failed', { cause: new Error('socket hang up (injected)') });
    }
    return response;
  };

  /** Requests whose path starts with `prefix` (after the host). */
  to(prefix: string, method?: string): Seen[] {
    return this.seen.filter((r) => r.path.startsWith(prefix) && (method === undefined || r.method === method));
  }
}

/** Makes `sleep` instantaneous and records the requested delays. */
export function recordedSleep(): { sleep: (ms: number) => Promise<void>; delays: number[] } {
  const delays: number[] = [];
  return {
    delays,
    sleep: async (ms: number) => {
      delays.push(ms);
    },
  };
}
