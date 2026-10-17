// The contents of the demo repository: a small TypeScript service, so the diffs in the screenshots read
// like real work. Everything here is plain text that is committed through the HTTP API.

export const INITIAL: Record<string, string> = {
  'README.md': `# webapp

Customer-facing web app: sessions, billing and the documentation site.

## Layout

- \`src/auth\`: sessions and the request middleware
- \`src/billing\`: invoices and tax
- \`docs\`: the public API guide
`,
  'package.json': `{
  "name": "webapp",
  "version": "2.3.0",
  "private": true,
  "scripts": { "test": "node --test tests/", "build": "tsc -p ." }
}
`,
  'src/index.ts': `import { createServer } from './server'

const port = Number(process.env.PORT ?? 3000)
createServer().listen(port, () => {
  console.log('webapp listening on ' + port)
})
`,
  'src/server.ts': `import { createServer as http } from 'node:http'
import { requireSession } from './auth/middleware'
import { invoiceTotal } from './billing/invoice'

export function createServer() {
  return http(async (req, res) => {
    const session = await requireSession(req)
    if (!session) {
      res.statusCode = 401
      return res.end('sign in first')
    }
    res.setHeader('content-type', 'application/json')
    res.end(JSON.stringify({ user: session.user, total: invoiceTotal(session.user, []) }))
  })
}
`,
  'src/auth/session.ts': `import { randomBytes } from 'node:crypto'

export interface Session {
  id: string
  user: string
  expiresAt: number
}

const sessions = new Map<string, Session>()
const TTL_MS = 30 * 60 * 1000

export function createSession(user: string): Session {
  const session = { id: randomBytes(16).toString('hex'), user, expiresAt: Date.now() + TTL_MS }
  sessions.set(session.id, session)
  return session
}

export function getSession(id: string): Session | undefined {
  const session = sessions.get(id)
  if (session && session.expiresAt < Date.now()) {
    sessions.delete(id)
    return undefined
  }
  return session
}
`,
  'src/auth/middleware.ts': `import type { IncomingMessage } from 'node:http'
import { getSession } from './session'

export async function requireSession(req: IncomingMessage) {
  const cookie = req.headers.cookie ?? ''
  const id = /(?:^|; )sid=([a-f0-9]+)/.exec(cookie)?.[1]
  return id ? getSession(id) : undefined
}
`,
  'src/billing/invoice.ts': `export interface Line {
  description: string
  cents: number
  quantity: number
}

export function invoiceTotal(user: string, lines: Line[]): number {
  const subtotal = lines.reduce((sum, l) => sum + l.cents * l.quantity, 0)
  return Math.round(subtotal * 1.2)
}
`,
  'src/billing/tax.ts': `export const VAT_RATE = 0.2

export function withVat(cents: number): number {
  return Math.round(cents * (1 + VAT_RATE))
}
`,
  'tests/invoice.test.ts': `import { test } from 'node:test'
import assert from 'node:assert/strict'
import { invoiceTotal } from '../src/billing/invoice'

test('adds VAT to the subtotal', () => {
  assert.equal(invoiceTotal('u', [{ description: 'seat', cents: 1000, quantity: 2 }]), 2400)
})
`,
  'docs/guide.md': `# Guide

Welcome. Sign in, then call the API with your session cookie.
`,
  'docs/api/sessions.md': `# Sessions API

TODO: document how sessions are created and how long they live.
`,
  '.github/workflows/ci.yml': `name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: npm test
`,
}

/** Proposal 1 (refactor-bot): pull the session store out of the module that also handles cookies. */
export const EXTRACT_STORE: Record<string, string> = {
  'src/auth/store.ts': `import type { Session } from './session'

/** Where sessions live. The in-memory store is the default; swap it for Redis in one place. */
export interface SessionStore {
  get(id: string): Session | undefined
  set(session: Session): void
  delete(id: string): void
}

export class MemoryStore implements SessionStore {
  private readonly sessions = new Map<string, Session>()

  get(id: string) {
    return this.sessions.get(id)
  }
  set(session: Session) {
    this.sessions.set(session.id, session)
  }
  delete(id: string) {
    this.sessions.delete(id)
  }
}
`,
  'src/auth/session.ts': `import { randomBytes } from 'node:crypto'
import { MemoryStore } from './store'
import type { SessionStore } from './store'

export interface Session {
  id: string
  user: string
  expiresAt: number
}

const TTL_MS = 30 * 60 * 1000
let store: SessionStore = new MemoryStore()

export function useStore(next: SessionStore) {
  store = next
}

export function createSession(user: string): Session {
  const session = { id: randomBytes(16).toString('hex'), user, expiresAt: Date.now() + TTL_MS }
  store.set(session)
  return session
}

export function getSession(id: string): Session | undefined {
  const session = store.get(id)
  if (session && session.expiresAt < Date.now()) {
    store.delete(id)
    return undefined
  }
  return session
}
`,
  'src/auth/middleware.ts': `import type { IncomingMessage } from 'node:http'
import { getSession } from './session'

const COOKIE = /(?:^|; )sid=([a-f0-9]+)/

export async function requireSession(req: IncomingMessage) {
  const id = COOKIE.exec(req.headers.cookie ?? '')?.[1]
  return id ? getSession(id) : undefined
}
`,
}

/** Proposal 2 (docs-writer): fill in the sessions page. Inside the autonomy envelope (docs/** only). */
export const DOCUMENT_SESSIONS: Record<string, string> = {
  'docs/api/sessions.md': `# Sessions API

A session is created when you sign in and identifies you on every later request.

## Lifetime

Sessions last **30 minutes** from creation. There is no sliding window: sign in again
after the session expires.

## Sending the session

Send the \`sid\` cookie that the sign-in response set:

\`\`\`http
GET /api/me HTTP/1.1
Cookie: sid=9f1c0e5a7b2d4c6e8a0b1c2d3e4f5a6b
\`\`\`

A missing or expired session is answered with \`401 sign in first\`.
`,
  'docs/guide.md': `# Guide

Welcome. Sign in, then call the API with your session cookie.

See [Sessions API](api/sessions.md) for how long a session lives and how to send it.
`,
}

/** Proposal 3 (alice): money in cents must not drift when the quantity is fractional. */
export const ROUNDING: Record<string, string> = {
  'src/billing/invoice.ts': `import { withVat } from './tax'

export interface Line {
  description: string
  cents: number
  quantity: number
}

export function invoiceTotal(user: string, lines: Line[]): number {
  // Round each line before adding VAT so the invoice total equals the sum of its printed lines.
  const subtotal = lines.reduce((sum, l) => sum + Math.round(l.cents * l.quantity), 0)
  return withVat(subtotal)
}
`,
  'tests/invoice.test.ts': `import { test } from 'node:test'
import assert from 'node:assert/strict'
import { invoiceTotal } from '../src/billing/invoice'

test('adds VAT to the subtotal', () => {
  assert.equal(invoiceTotal('u', [{ description: 'seat', cents: 1000, quantity: 2 }]), 2400)
})

test('rounds each line, not the total', () => {
  const lines = [
    { description: 'metered', cents: 333, quantity: 1.5 },
    { description: 'metered', cents: 333, quantity: 1.5 },
  ]
  assert.equal(invoiceTotal('u', lines), 1200)
})
`,
}

/** Proposal 4 (test-fixer): make the flaky test deterministic. Overlaps proposal 3 on the test file. */
export const FLAKY_TEST: Record<string, string> = {
  'tests/invoice.test.ts': `import { test } from 'node:test'
import assert from 'node:assert/strict'
import { invoiceTotal } from '../src/billing/invoice'

// The old test compared against a total computed with the same code, so it passed or failed by accident.
test('adds VAT to the subtotal', () => {
  assert.equal(invoiceTotal('u', [{ description: 'seat', cents: 1000, quantity: 2 }]), 2400)
})
`,
  'src/billing/tax.ts': `export const VAT_RATE = 0.2

/** Integer cents in, integer cents out; never floats across the boundary. */
export function withVat(cents: number): number {
  return Math.round((cents * (100 + VAT_RATE * 100)) / 100)
}
`,
}

export const GREETING_FRIENDLY: Record<string, string> = {
  'src/index.ts': `import { createServer } from './server'

const port = Number(process.env.PORT ?? 3000)
createServer().listen(port, () => {
  console.log('Hi! webapp is up on port ' + port)
})
`,
}

export const GREETING_FORMAL: Record<string, string> = {
  'src/index.ts': `import { createServer } from './server'

const port = Number(process.env.PORT ?? 3000)
createServer().listen(port, () => {
  console.log('webapp: ready to accept connections on port ' + port)
})
`,
}

export const WORKFLOW_BUMP: Record<string, string> = {
  '.github/workflows/ci.yml': `name: ci
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: 22
      - run: npm test
`,
}

export const HEALTHZ: Record<string, string> = {
  'src/server.ts': `import { createServer as http } from 'node:http'
import { requireSession } from './auth/middleware'
import { invoiceTotal } from './billing/invoice'

export function createServer() {
  return http(async (req, res) => {
    if (req.url === '/healthz') {
      res.statusCode = 200
      return res.end('ok')
    }
    const session = await requireSession(req)
    if (!session) {
      res.statusCode = 401
      return res.end('sign in first')
    }
    res.setHeader('content-type', 'application/json')
    res.end(JSON.stringify({ user: session.user, total: invoiceTotal(session.user, []) }))
  })
}
`,
}

export const README_DEV: Record<string, string> = {
  'README.md': `# webapp

Customer-facing web app: sessions, billing and the documentation site.

## Layout

- \`src/auth\`: sessions and the request middleware
- \`src/billing\`: invoices and tax
- \`docs\`: the public API guide

## Local development

\`\`\`sh
npm install
PORT=3000 npm run build && node dist/index.js
npm test
\`\`\`

The server answers \`/healthz\` without a session, so a load balancer can probe it.
`,
}

export const NODE22_PACKAGE: Record<string, string> = {
  'package.json': `{
  "name": "webapp",
  "version": "2.3.0",
  "private": true,
  "engines": { "node": ">=22" },
  "scripts": { "test": "node --test tests/", "build": "tsc -p ." }
}
`,
}

/** A tiny PNG (1×1) to show how binary changes are handled. */
export const PNG_1X1 = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==',
  'base64',
)
