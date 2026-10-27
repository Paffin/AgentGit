// Building blocks for the end-to-end world: who exists, how to commit and propose through the HTTP API,
// and two small background workers that stand in for CI and for running agents. Everything goes through
// the public API of a real ag-server; nothing here reaches into the server's storage.
import { createHash, randomBytes } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { Api } from './http.ts'
import type { Reply } from './http.ts'
import { b64, keyFromSeed, signAttestation } from './signing.ts'
import type { KeyPair } from './signing.ts'

/** Everything in the demo lives under this organisation, so one role grant covers every repository. */
export const ORG = 'acme'

export const PEOPLE = {
  dana: { subject: 'user:dana', role: 'admin', extra: ['writer'], about: 'Platform lead' },
  alice: { subject: 'user:alice', role: 'writer', extra: [], about: 'Backend engineer' },
  bob: { subject: 'user:bob', role: 'reviewer', extra: [], about: 'Reviewer' },
  carol: { subject: 'user:carol', role: 'integrator', extra: [], about: 'Release manager' },
  erin: { subject: 'user:erin', role: 'reader', extra: [], about: 'Read-only stakeholder' },
  ci: { subject: 'service:ci', role: 'runner', extra: [], about: 'CI runner' },
} as const

export const AGENTS = ['refactor-bot', 'docs-writer', 'test-fixer', 'dep-updater'] as const
export type AgentName = (typeof AGENTS)[number]
export type PersonName = keyof typeof PEOPLE
export type Who = PersonName | AgentName | 'admin'

/** The CI service's signing key. Deterministic so a policy written by a test can trust it. */
export const CI_KEY: KeyPair = keyFromSeed('agentgit-e2e/service:ci')
export const agentKey = (name: string): KeyPair => keyFromSeed(`agentgit-e2e/agent:${name}`)

export const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))
export const suffix = () => randomBytes(3).toString('hex')

/** The signed-in identities of one running server. */
export class Cast {
  readonly baseURL: string
  readonly tokens: Record<string, string>

  constructor(baseURL: string, tokens: Record<string, string>) {
    this.baseURL = baseURL
    this.tokens = tokens
  }

  as(who: Who | (string & {})): Api {
    const token = this.tokens[who]
    if (!token) throw new Error(`no token for ${who}`)
    return new Api(this.baseURL, token)
  }

  token(who: Who | (string & {})): string {
    const token = this.tokens[who]
    if (!token) throw new Error(`no token for ${who}`)
    return token
  }
}

/** Grants the roles and mints one long-lived credential per persona. Idempotent per server. */
export async function bootstrapCast(baseURL: string, adminToken: string): Promise<Cast> {
  const admin = new Api(baseURL, adminToken)
  const tokens: Record<string, string> = { admin: adminToken }
  const grant = (subject: string, role: string) =>
    admin.ok_(
      admin.post('/v1/roles', { subject, repo: `${ORG}/*`, role }),
      `grant ${role} to ${subject}`,
    )
  const mint = async (key: string, subject: string, description: string, repos?: string[]) => {
    const res = await admin.ok_(
      admin.post('/v1/credentials', {
        subject,
        description,
        // The server caps agent credentials at one hour; the suite runs far shorter than that.
        ttl_secs: repos ? 3_600 : 86_400,
        ...(repos ? { scope: { repos } } : {}),
      }),
      `mint ${subject}`,
    )
    tokens[key] = res.token
  }
  for (const [name, p] of Object.entries(PEOPLE)) {
    await grant(p.subject, p.role)
    for (const extra of p.extra) await grant(p.subject, extra)
    await mint(name, p.subject, `${p.about} (e2e)`)
  }
  for (const name of AGENTS) {
    await grant(`agent:${name}`, 'writer')
    await mint(name, `agent:${name}`, `${name} automation (e2e)`, [`${ORG}/*`])
  }
  return new Cast(baseURL, tokens)
}

// ---- committing through workspaces ---------------------------------------------------------------

export type FileMap = Record<string, string | Uint8Array | null>

function changesOf(files: FileMap) {
  return Object.entries(files).map(([path, content]) =>
    content === null ? { path, op: 'delete' } : { path, op: 'put', content: b64(content) },
  )
}

const asRun = (runId?: string): Record<string, string> => (runId ? { 'ag-run': runId } : {})

export interface Committed {
  workspace: string
  head: string
  base: string
}

/** Opens a workspace on the stream and commits `files` on top of it (null deletes a file). */
export async function commitFiles(
  api: Api,
  repo: string,
  files: FileMap,
  message: string,
  opts: { stream?: string; fromRev?: string; runId?: string; workspace?: string } = {},
): Promise<Committed> {
  const name = opts.workspace ?? `ws-${suffix()}${suffix()}`
  const created = await api.ok_(
    api.call(
      'POST',
      `/v1/repos/${repo}/workspaces`,
      { name, stream: opts.stream ?? 'main', ...(opts.fromRev ? { from_rev: opts.fromRev } : {}) },
      asRun(opts.runId),
    ),
    `create workspace ${name}`,
  )
  const changed = await amend(api, repo, name, files, message, opts.runId)
  return { workspace: name, head: changed.head, base: created.base }
}

/** Another commit on an existing workspace. */
export async function amend(
  api: Api,
  repo: string,
  workspace: string,
  files: FileMap,
  message: string,
  runId?: string,
): Promise<{ head: string }> {
  return api.ok_(
    api.call(
      'POST',
      `/v1/repos/${repo}/workspaces/${workspace}/changes`,
      { message, changes: changesOf(files) },
      asRun(runId),
    ),
    `commit "${message}"`,
  )
}

export async function openProposal(
  api: Api,
  repo: string,
  p: {
    head: string
    title: string
    description?: string
    taskId?: string
    stream?: string
    runId?: string
  },
): Promise<{ id: string; head: string; author: string }> {
  return api.ok_(
    api.call(
      'POST',
      `/v1/repos/${repo}/proposals`,
      {
        stream: p.stream ?? 'main',
        head: p.head,
        title: p.title,
        description: p.description ?? '',
        ...(p.taskId ? { task_id: p.taskId } : {}),
      },
      asRun(p.runId),
    ),
    `propose "${p.title}"`,
  )
}

/** Integrates and insists that it lands; used for the history that has to exist before the story starts. */
export async function integrate(
  api: Api,
  repo: string,
  proposal: string,
  fencingTokens: number[] = [],
): Promise<{ new_head: string; strategy: string }> {
  const res = await api.ok_(
    api.post(
      `/v1/repos/${repo}/proposals/${proposal}/integrate`,
      fencingTokens.length ? { fencing_tokens: fencingTokens } : {},
    ),
    `integrate ${proposal}`,
  )
  if (res.outcome?.status !== 'integrated') {
    throw new Error(`integrate ${proposal}: ${JSON.stringify(res.outcome)}`)
  }
  return res.outcome
}

export async function review(
  api: Api,
  repo: string,
  proposal: string,
  decision: 'approve' | 'request_changes' | 'comment',
  body = '',
) {
  return api.ok_(
    api.post(`/v1/repos/${repo}/proposals/${proposal}/reviews`, { decision, body }),
    `review ${proposal}`,
  )
}

export const prepare = (api: Api, repo: string, proposal: string) =>
  api.ok_(api.post(`/v1/repos/${repo}/proposals/${proposal}/prepare`, {}), `prepare ${proposal}`)

// ---- policy --------------------------------------------------------------------------------------

export interface PolicyOptions {
  approvals?: { min: number; min_for_agents?: number; forbid_self_approval?: boolean }
  requireTests?: boolean
  protectedPaths?: { glob: string; min_approvals: number }[]
  autonomy?: { allowed_paths: string[]; max_files: number; auto_integrate?: boolean }
  requireProvenance?: boolean
}

export function policyDocument(o: PolicyOptions = {}): Record<string, unknown> {
  const doc: Record<string, unknown> = {
    trusted_issuers: [{ identity: PEOPLE.ci.subject, public_key: CI_KEY.publicHex }],
  }
  if (o.requireTests !== false) {
    doc.required_validations = [
      {
        check: 'tests',
        issuers: [PEOPLE.ci.subject],
        run: { argv: ['npm', 'test'], timeout_secs: 300 },
      },
    ]
  }
  if (o.approvals) doc.approvals = o.approvals
  if (o.protectedPaths) doc.protected_paths = o.protectedPaths
  if (o.autonomy) doc.agent_autonomy = o.autonomy
  if (o.requireProvenance) doc.require_provenance_for_agents = true
  return doc
}

export async function setPolicy(
  api: Api,
  repo: string,
  doc: Record<string, unknown>,
  stream = 'main',
) {
  return api.ok_(
    api.put(`/v1/repos/${repo}/policy`, { stream, document: JSON.stringify(doc) }),
    'set policy',
  )
}

// ---- CI stand-in ---------------------------------------------------------------------------------

/**
 * The CI stand-in behaves like a real runner against the real execution plane: it claims `validate`
 * jobs over HTTP, checks the job's tree id, and submits a signed attestation for exactly that tree.
 * It does NOT execute the command. A test can make it fail a proposal by adding the id to the rules file.
 */
export interface CiRules {
  failProposals?: string[]
  /** Pretend the check takes this long. */
  workMs?: number
}

export function readRules(runtimeDir: string): CiRules {
  const file = join(runtimeDir, 'ci-rules.json')
  if (!existsSync(file)) return {}
  try {
    return JSON.parse(readFileSync(file, 'utf8')) as CiRules
  } catch {
    return {}
  }
}

export interface Worker {
  stop(): Promise<void>
  /** Jobs completed so far. */
  readonly done: number
}

export function startCiWorker(opts: {
  baseURL: string
  token: string
  runtimeDir: string
  pollMs?: number
}): Worker {
  const api = new Api(opts.baseURL, opts.token)
  const workerId = 'ci-e2e-1'
  let stopped = false
  let done = 0
  const loop = (async () => {
    while (!stopped) {
      let job: any = null
      try {
        const res = await api.post('/v1/jobs/claim', {
          repo: '',
          kinds: ['validate'],
          worker: workerId,
          lease_ms: 60_000,
          wait_ms: 0,
        })
        job = res.json?.job ?? null
      } catch {
        /* the server is shutting down or restarting */
      }
      if (!job) {
        await sleep(opts.pollMs ?? 250)
        continue
      }
      const rules = readRules(opts.runtimeDir)
      const v = job.validation ?? {}
      await sleep(rules.workMs ?? 600)
      const failed = (rules.failProposals ?? []).includes(v.proposal)
      const attestation = signAttestation(CI_KEY, {
        treeId: v.tree_id,
        check: v.check,
        result: failed ? 'fail' : 'pass',
        policyVersion: Number(v.policy_version ?? 0),
        environmentDigest:
          v.required_environment_digest ||
          `sha256:${createHash('sha256').update('node22/linux/x86_64').digest('hex')}`,
        issuer: PEOPLE.ci.subject,
        issuedAtMs: Date.now(),
      })
      try {
        await api.post(`/v1/repos/${job.repo}/jobs/${job.id}/complete`, {
          worker: workerId,
          attestation,
          result: {
            exit_code: failed ? 1 : 0,
            duration_ms: rules.workMs ?? 600,
            summary: failed ? '2 passing, 1 failing' : '14 passing',
          },
        })
        done++
      } catch {
        /* the next claim will pick the job up again */
      }
    }
  })()
  return {
    async stop() {
      stopped = true
      await loop
    },
    get done() {
      return done
    },
  }
}

// ---- agent heartbeats ----------------------------------------------------------------------------

export interface Beating {
  token: string
  repo: string
  runId: string
}

export interface HeartbeatKeeper {
  add(b: Beating): void
  remove(runId: string): void
  stop(): Promise<void>
}

function pauseUntilStopped(ms: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.resolve()
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer)
      signal.removeEventListener('abort', finish)
      resolve()
    }
    const timer = setTimeout(finish, ms)
    signal.addEventListener('abort', finish)
  })
}

/** Keeps the listed runs alive, the way a working agent would. A run that is not listed goes quiet. */
export function startHeartbeats(baseURL: string, everyMs = 2000): HeartbeatKeeper {
  const beating = new Map<string, Beating>()
  const stopping = new AbortController()
  const loop = (async () => {
    while (!stopping.signal.aborted) {
      await Promise.all(
        [...beating.values()].map(async (b) => {
          try {
            await new Api(baseURL, b.token).post(
              `/v1/repos/${b.repo}/runs/${b.runId}/heartbeat`,
              {},
            )
          } catch {
            /* ignore: the next beat retries */
          }
        }),
      )
      await pauseUntilStopped(everyMs, stopping.signal)
    }
  })()
  return {
    add: (b) => void beating.set(b.runId, b),
    remove: (runId) => void beating.delete(runId),
    async stop() {
      stopping.abort()
      await loop
    },
  }
}

// ---- agents in a repository ----------------------------------------------------------------------

export interface AgentRun {
  runId: string
  api: Api
  name: AgentName
  /** Calls the API as this run (the `ag-run` header). */
  as(method: string, path: string, body?: unknown): Promise<Reply>
}

/** Registers the agent in `repo` (once) and starts a run, optionally on a task. */
export async function startRun(
  cast: Cast,
  repo: string,
  name: AgentName,
  taskId?: string,
): Promise<AgentRun> {
  const api = cast.as(name)
  // Registering twice is harmless: the second answer is an error we ignore.
  await api.post(`/v1/repos/${repo}/agents`, { name, public_key: agentKey(name).publicHex })
  const run = await api.ok_(
    api.post(`/v1/repos/${repo}/runs`, { agent: name, ...(taskId ? { task_id: taskId } : {}) }),
    `start run of ${name}`,
  )
  return {
    runId: run.run_id,
    api,
    name,
    as: (method, path, body) => api.call(method, path, body, { 'ag-run': run.run_id }),
  }
}

// ---- waiting -------------------------------------------------------------------------------------

export async function waitFor<T>(
  what: string,
  probe: () => Promise<T | null | undefined | false>,
  timeoutMs = 20_000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const v = await probe()
    if (v) return v
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`)
    await sleep(150)
  }
}

/** Asks for the validation jobs and waits until every required check has a result on the exact tree. */
export async function validated(api: Api, repo: string, proposal: string, timeoutMs = 30_000) {
  await api.ok_(
    api.post(`/v1/repos/${repo}/proposals/${proposal}/validate`, {}),
    'request validation',
  )
  return waitFor(
    `validation of ${proposal}`,
    async () => {
      const prep = await prepare(api, repo, proposal)
      const vs: any[] = prep.validations ?? []
      return vs.length > 0 && vs.every((x) => x.state === 'passed' || x.state === 'failed')
        ? prep
        : null
    },
    timeoutMs,
  )
}

// ---- a fresh, small repository for tests that change things ---------------------------------------

export const SKELETON: FileMap = {
  'README.md': '# demo\n\nA tiny repository for one test.\n',
  'src/app.ts': 'export const answer = 41\n',
  'src/util.ts': 'export const double = (n: number) => n * 2\n',
  'docs/notes.md': '# Notes\n\nNothing yet.\n',
}

/** Creates `acme/<label>-<random>` with one landed commit. The caller decides about policy. */
export async function freshRepo(
  cast: Cast,
  label: string,
  files: FileMap = SKELETON,
): Promise<{ repo: string; head: string }> {
  const repo = `${ORG}/${label}-${suffix()}`
  await cast.as('admin').ok_(cast.as('admin').post('/v1/repos', { name: repo }), `create ${repo}`)
  const c = await commitFiles(cast.as('alice'), repo, files, 'Import the skeleton')
  const p = await openProposal(cast.as('alice'), repo, {
    head: c.head,
    title: 'Import the skeleton',
  })
  const out = await integrate(cast.as('carol'), repo, p.id)
  return { repo, head: out.new_head }
}
