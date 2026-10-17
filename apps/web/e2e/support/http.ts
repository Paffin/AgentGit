// A tiny typed-enough HTTP helper for seeding and probing a real AgentGit server from tests.
// It deliberately does not share code with the UI's client: tests must not depend on the thing they test.

export interface Reply<T = any> {
  status: number
  ok: boolean
  json: T
  headers: Headers
}

export class Api {
  readonly base: string
  readonly token: string

  constructor(base: string, token: string) {
    this.base = base
    this.token = token
  }

  as(token: string): Api {
    return new Api(this.base, token)
  }

  async call<T = any>(
    method: string,
    path: string,
    body?: unknown,
    headers: Record<string, string> = {},
  ): Promise<Reply<T>> {
    // A rate-limited answer (429) is waited out and repeated, as any well-behaved client does.
    for (let attempt = 0; ; attempt++) {
      const res = await fetch(this.base + path, {
        method,
        headers: {
          ...(this.token ? { authorization: `Bearer ${this.token}` } : {}),
          ...(body === undefined ? {} : { 'content-type': 'application/json' }),
          ...headers,
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      })
      const text = await res.text()
      const json = parseJson(text)
      if (res.status === 429 && attempt < 8) {
        const wait =
          Number(json?.error?.retry_after_ms) ||
          Number(res.headers.get('retry-after')) * 1000 ||
          250
        await new Promise((r) => setTimeout(r, Math.min(wait, 5000)))
        continue
      }
      return { status: res.status, ok: res.ok, json, headers: res.headers }
    }
  }

  get<T = any>(path: string, headers?: Record<string, string>) {
    return this.call<T>('GET', path, undefined, headers)
  }
  post<T = any>(path: string, body: unknown = {}, headers?: Record<string, string>) {
    return this.call<T>('POST', path, body, headers)
  }
  put<T = any>(path: string, body: unknown, headers?: Record<string, string>) {
    return this.call<T>('PUT', path, body, headers)
  }
  patch<T = any>(path: string, body: unknown, headers?: Record<string, string>) {
    return this.call<T>('PATCH', path, body, headers)
  }
  del<T = any>(path: string, headers?: Record<string, string>) {
    return this.call<T>('DELETE', path, undefined, headers)
  }

  /** Like `call`, but throws with the server's error when the status is not 2xx. */
  async ok_<T = any>(r: Promise<Reply<T>>, what: string): Promise<T> {
    const reply = await r
    if (!reply.ok) {
      throw new Error(`${what}: HTTP ${reply.status} ${JSON.stringify(reply.json)}`)
    }
    return reply.json
  }
}

function parseJson(text: string): any {
  if (!text) return null
  try {
    return JSON.parse(text)
  } catch {
    return text
  }
}
