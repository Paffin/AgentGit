// The demo world: a small team and four agents working on `acme/webapp`. It is built purely through the
// HTTP API of a real server, in the order a real week would have produced it, so the live board has
// something true to show: runs (one of them gone quiet), leases with fencing tokens and an overlap,
// a task graph, proposals in different states of the four-part verdict, a queue, a conflict.
//
// The same file builds a few small side repositories (an empty one, one full of hostile text).
import {
  DOCUMENT_SESSIONS,
  EXTRACT_STORE,
  FLAKY_TEST,
  GREETING_FORMAL,
  GREETING_FRIENDLY,
  HEALTHZ,
  INITIAL,
  NODE22_PACKAGE,
  PNG_1X1,
  README_DEV,
  ROUNDING,
  WORKFLOW_BUMP,
} from './files.ts'
import {
  agentKey,
  amend,
  commitFiles,
  integrate,
  openProposal,
  policyDocument,
  prepare,
  review,
  setPolicy,
  validated,
  waitFor,
} from './world.ts'
import type { AgentName, Cast, HeartbeatKeeper } from './world.ts'
import { signProvenance } from './signing.ts'
import { failChecksFor } from './state.ts'
import type { Api } from './http.ts'

export const DEMO_REPO = 'acme/webapp'
export const EMPTY_REPO = 'acme/mobile'
export const HOSTILE_REPO = 'acme/xss-lab'

export interface Demo {
  repo: string
  proposals: {
    /** refactor-bot: validated, needs an approval, holds an exclusive lease. */
    extract: string
    /** docs-writer: approved and validated, sitting in the merge queue. */
    docs: string
    /** alice: not validated, and its approval is stale because she pushed again. */
    rounding: string
    /** test-fixer: a failing validation and a reviewer asking for changes. */
    flaky: string
    /** dana: conflicts with the one that landed before it. */
    formal: string
    /** dep-updater: touches a protected path, one of two approvals. */
    node: string
    /** alice: the friendlier greeting that landed (the other side of the conflict). */
    greeting: string
  }
  tasks: Record<
    'import' | 'extract' | 'docs' | 'flaky' | 'node' | 'redis' | 'metrics' | 'runbook',
    string
  >
  runs: Record<AgentName, string>
  leases: { auth: string; billing: string; docs: string; ci: string; exploring: string }
  fencing: { auth: number; billing: number }
  headBeforeStory: string
  pin: string
}

const log = (msg: string) => {
  if (process.env.AG_E2E_VERBOSE) process.stdout.write(`seed: ${msg}\n`)
}

async function must<T>(
  p: Promise<{ ok: boolean; status: number; json: T }>,
  what: string,
): Promise<T> {
  const r = await p
  if (!r.ok) throw new Error(`${what}: HTTP ${r.status} ${JSON.stringify(r.json)}`)
  return r.json
}

/** Provenance a well-behaved agent attaches to its change: signed with its own registered key. */
async function attach(
  bot: Api,
  repo: string,
  name: AgentName,
  runId: string,
  taskId: string,
  changeset: string,
  p: { model: string; rationale: string; checks: string[]; inputs: string[]; temperature?: string },
) {
  const signed = signProvenance(agentKey(name), {
    changeset,
    agent: `agent:${name}#${runId}`,
    runId,
    taskId,
    model: p.model,
    rationale: p.rationale,
    parameters: { temperature: p.temperature ?? '0.2', max_steps: '40' },
    checksRun: p.checks,
    inputs: p.inputs,
    createdAtMs: Date.now(),
  })
  await must(
    bot.call('POST', `/v1/repos/${repo}/provenance`, signed, { 'ag-run': runId }),
    `provenance for ${name}`,
  )
}

export async function seedDemo(cast: Cast, beats: HeartbeatKeeper): Promise<Demo> {
  const R = `/v1/repos/${DEMO_REPO}`
  const admin = cast.as('admin')
  const dana = cast.as('dana')
  const alice = cast.as('alice')
  const bob = cast.as('bob')
  const carol = cast.as('carol')

  // ---- the repository, its agents ------------------------------------------------------------------
  log('repository and agents')
  await must(
    admin.post('/v1/repos', {
      name: DEMO_REPO,
      description: 'Customer-facing web app: sessions, billing and the documentation site.',
    }),
    'create repo',
  )
  for (const name of ['refactor-bot', 'docs-writer', 'test-fixer', 'dep-updater'] as const) {
    await must(
      cast.as(name).post(`${R}/agents`, { name, public_key: agentKey(name).publicHex }),
      `register ${name}`,
    )
  }

  // ---- history before the story: an import, a fast-forward and a merge ---------------------------------
  log('history')
  const first = await commitFiles(alice, DEMO_REPO, INITIAL, 'Import the webapp skeleton')
  const firstProposal = await openProposal(alice, DEMO_REPO, {
    head: first.head,
    title: 'Import the webapp skeleton',
    description: 'Moves the prototype into AgentGit with its history squashed.',
  })
  await integrate(carol, DEMO_REPO, firstProposal.id)

  // Two people branch from the same commit; the second one lands as a real merge.
  const health = await commitFiles(alice, DEMO_REPO, HEALTHZ, 'Answer /healthz without a session')
  const readme = await commitFiles(dana, DEMO_REPO, README_DEV, 'Document local development')
  const healthProposal = await openProposal(alice, DEMO_REPO, {
    head: health.head,
    title: 'Answer /healthz without a session',
    description: 'Load balancers probe /healthz; it must not need a cookie.',
  })
  const readmeProposal = await openProposal(dana, DEMO_REPO, {
    head: readme.head,
    title: 'Document local development',
  })
  await integrate(carol, DEMO_REPO, healthProposal.id)
  await integrate(carol, DEMO_REPO, readmeProposal.id)

  const headBeforeStory = (await must(alice.get(`${R}/streams/head?stream=main`), 'head'))
    .head as string
  const pin = 'v2.3.0'
  await must(
    admin.post(`${R}/pins`, {
      name: pin,
      rev: 'main',
      reason: 'Release 2.3.0 (sessions + billing)',
    }),
    'pin',
  )

  // ---- policy: two versions, so the editor has a history to diff -----------------------------------------
  log('policy')
  await setPolicy(
    dana,
    DEMO_REPO,
    policyDocument({ approvals: { min: 1, forbid_self_approval: true } }),
  )
  await setPolicy(
    dana,
    DEMO_REPO,
    policyDocument({
      approvals: { min: 1, min_for_agents: 1, forbid_self_approval: true },
      protectedPaths: [{ glob: '.github/**', min_approvals: 2 }],
      autonomy: { allowed_paths: ['docs/**'], max_files: 5, auto_integrate: true },
      requireProvenance: true,
    }),
  )

  // ---- the task graph -------------------------------------------------------------------------------
  log('tasks')
  const task = async (t: {
    title: string
    description?: string
    scope?: string[]
    depends_on?: string[]
  }): Promise<string> => (await must(alice.post(`${R}/tasks`, t), `task ${t.title}`)).id
  const tImport = await task({ title: 'Import the webapp skeleton', scope: ['**'] })
  await must(alice.post(`${R}/tasks/${tImport}/finish`, { state: 'done' }), 'finish import task')
  const tExtract = await task({
    title: 'Extract the session store',
    description: 'Sessions live in a module-level Map today. Put them behind an interface.',
    scope: ['src/auth/**'],
  })
  const tDocs = await task({
    title: 'Document the sessions API',
    description: 'Say how long a session lives and how to send it.',
    scope: ['docs/api/**'],
  })
  const tFlaky = await task({
    title: 'Fix the flaky invoice test',
    description: 'tests/invoice.test.ts fails about one run in ten.',
    scope: ['tests/**', 'src/billing/**'],
  })
  const tNode = await task({
    title: 'Bump Node to 22 in CI',
    description: 'Node 20 leaves maintenance next spring.',
    scope: ['.github/**', 'package.json'],
  })
  const tRedis = await task({
    title: 'Migrate sessions to Redis',
    description: 'Needs the store interface first.',
    depends_on: [tExtract],
  })
  const tMetrics = await task({ title: 'Add session expiry metrics', depends_on: [tExtract] })
  const tRunbook = await task({ title: 'Write the Redis runbook', depends_on: [tRedis] })
  // A dependency added after the fact (the runbook also waits for the metrics).
  await must(
    alice.post(`${R}/tasks/${tRunbook}/dependencies`, { depends_on: tMetrics }),
    'add dependency',
  )

  // ---- runs, leases ---------------------------------------------------------------------------------------
  log('runs and leases')
  const bots = {
    'refactor-bot': cast.as('refactor-bot'),
    'docs-writer': cast.as('docs-writer'),
    'test-fixer': cast.as('test-fixer'),
    'dep-updater': cast.as('dep-updater'),
  } as const
  // Earlier runs that are over, so the history of runs is not just the four active ones.
  const past1 = await must(
    bots['refactor-bot'].post(`${R}/runs`, { agent: 'refactor-bot' }),
    'past run',
  )
  await must(
    bots['refactor-bot'].post(`${R}/runs/${past1.run_id}/complete`, {
      status: 'completed',
      summary: 'Removed three unused exports from src/billing.',
    }),
    'complete past run',
  )
  const past2 = await must(
    bots['dep-updater'].post(`${R}/runs`, { agent: 'dep-updater' }),
    'past run',
  )
  await must(
    bots['dep-updater'].post(`${R}/runs/${past2.run_id}/complete`, {
      status: 'failed',
      summary: 'npm audit fix could not resolve a peer dependency conflict.',
    }),
    'complete past run',
  )

  const runs = {} as Demo['runs']
  const start = async (name: AgentName, taskId: string) => {
    const run = await must(
      bots[name].post(`${R}/runs`, { agent: name, task_id: taskId }),
      `start ${name}`,
    )
    runs[name] = run.run_id
    return run.run_id as string
  }
  const rBot = await start('refactor-bot', tExtract)
  const rDocs = await start('docs-writer', tDocs)
  const rTest = await start('test-fixer', tFlaky)
  const rDeps = await start('dep-updater', tNode)
  // The first three keep working; dep-updater never heartbeats again and goes quiet on the board.
  for (const [name, runId] of [
    ['refactor-bot', rBot],
    ['docs-writer', rDocs],
    ['test-fixer', rTest],
  ] as const) {
    beats.add({ token: cast.token(name), repo: DEMO_REPO, runId })
  }

  const lease = async (
    who: Api,
    runId: string | undefined,
    scope: string,
    o: { exclusive?: boolean; taskId?: string; minutes?: number } = {},
  ) =>
    must(
      who.call(
        'POST',
        `${R}/leases`,
        {
          scope,
          exclusive: o.exclusive ?? false,
          ttl_ms: (o.minutes ?? 50) * 60_000,
          ...(o.taskId ? { task_id: o.taskId } : {}),
        },
        runId ? { 'ag-run': runId } : {},
      ),
      `lease ${scope}`,
    )
  const lAuth = await lease(bots['refactor-bot'], rBot, 'src/auth/**', {
    exclusive: true,
    taskId: tExtract,
  })
  const lBilling = await lease(bots['test-fixer'], rTest, 'src/billing/**', {
    exclusive: true,
    taskId: tFlaky,
  })
  const lDocs = await lease(bots['docs-writer'], rDocs, 'docs/api/**', { taskId: tDocs })
  const lCi = await lease(bots['dep-updater'], rDeps, '.github/**', {
    exclusive: true,
    taskId: tNode,
  })
  // Alice is "just looking around" in all of src/: a soft lease that overlaps two exclusive ones.
  const lLook = await lease(alice, undefined, 'src/**', { minutes: 30 })

  // ---- the five proposals -----------------------------------------------------------------------------------
  log('commits')
  // Everything below branches from the same head, so what lands first decides what conflicts.
  const wsGreeting = await commitFiles(
    alice,
    DEMO_REPO,
    GREETING_FRIENDLY,
    'Friendlier startup message',
  )
  const wsFormal = await commitFiles(
    dana,
    DEMO_REPO,
    GREETING_FORMAL,
    'Print a formal startup line',
  )
  const wsExtract = await commitFiles(
    bots['refactor-bot'],
    DEMO_REPO,
    EXTRACT_STORE,
    'Extract the session store',
    {
      runId: rBot,
    },
  )
  const wsDocs = await commitFiles(
    bots['docs-writer'],
    DEMO_REPO,
    { ...DOCUMENT_SESSIONS, 'docs/assets/session-lifetime.png': PNG_1X1 },
    'Document how sessions are created and how long they live',
    { runId: rDocs },
  )
  const wsRounding = await commitFiles(
    alice,
    DEMO_REPO,
    { 'src/billing/invoice.ts': ROUNDING['src/billing/invoice.ts']! },
    'Round each invoice line before VAT',
  )
  const wsFlaky = await commitFiles(
    bots['test-fixer'],
    DEMO_REPO,
    FLAKY_TEST,
    'Make the invoice test deterministic',
    {
      runId: rTest,
    },
  )
  const wsNode = await commitFiles(
    bots['dep-updater'],
    DEMO_REPO,
    { ...WORKFLOW_BUMP, ...NODE22_PACKAGE },
    'Run CI on Node 22',
    { runId: rDeps },
  )

  log('proposals')
  const pGreeting = await openProposal(alice, DEMO_REPO, {
    head: wsGreeting.head,
    title: 'Friendlier startup message',
    description: 'Support asked for a log line a human can read at 3am.',
  })
  const pFormal = await openProposal(dana, DEMO_REPO, {
    head: wsFormal.head,
    title: 'Print a formal startup line',
    description: 'The log shipper greps for "ready to accept connections".',
  })
  const pExtract = await openProposal(bots['refactor-bot'], DEMO_REPO, {
    head: wsExtract.head,
    title: 'Extract the session store',
    description:
      'Moves the session Map behind a SessionStore interface so Redis can replace it in one place.\n\nNo behaviour change: the default is still an in-memory store with the same 30 minute TTL.',
    taskId: tExtract,
    runId: rBot,
  })
  const pDocs = await openProposal(bots['docs-writer'], DEMO_REPO, {
    head: wsDocs.head,
    title: 'Document how sessions are created and how long they live',
    description: 'Fills in docs/api/sessions.md and links it from the guide.',
    taskId: tDocs,
    runId: rDocs,
  })
  const pRounding = await openProposal(alice, DEMO_REPO, {
    head: wsRounding.head,
    title: 'Round each invoice line before VAT',
    description: 'Totals drifted by a cent when quantities were fractional.',
  })
  const pFlaky = await openProposal(bots['test-fixer'], DEMO_REPO, {
    head: wsFlaky.head,
    title: 'Make the invoice test deterministic',
    description: 'The test compared a total with one computed by the same code.',
    taskId: tFlaky,
    runId: rTest,
  })
  const pNode = await openProposal(bots['dep-updater'], DEMO_REPO, {
    head: wsNode.head,
    title: 'Run CI on Node 22',
    description: 'Node 20 leaves maintenance next spring.',
    taskId: tNode,
    runId: rDeps,
  })

  // Signed provenance for the agents' work (agents without it are blocked by the policy).
  log('provenance')
  await attach(bots['refactor-bot'], DEMO_REPO, 'refactor-bot', rBot, tExtract, wsExtract.head, {
    model: 'example-coder-large',
    rationale:
      'The session logic mixed storage with cookie parsing. Splitting the store out makes the Redis migration (T5) a one-file change.',
    checks: ['npm test', 'tsc --noEmit'],
    inputs: ['docs/api/sessions.md', 'src/auth/session.ts', 'issue #214'],
  })
  await attach(bots['docs-writer'], DEMO_REPO, 'docs-writer', rDocs, tDocs, wsDocs.head, {
    model: 'example-writer-small',
    rationale:
      'Read src/auth/session.ts for the TTL and middleware.ts for the cookie name; documented only what the code does.',
    checks: ['markdown link check'],
    inputs: ['src/auth/session.ts', 'src/auth/middleware.ts'],
    temperature: '0.4',
  })
  await attach(bots['test-fixer'], DEMO_REPO, 'test-fixer', rTest, tFlaky, wsFlaky.head, {
    model: 'example-coder-large',
    rationale:
      'The old assertion recomputed the total with the code under test. Replaced it with a literal and made withVat integer-only.',
    checks: ['node --test tests/ (20 runs)'],
    inputs: ['tests/invoice.test.ts', 'CI run #4812'],
    temperature: '0.1',
  })
  await attach(bots['dep-updater'], DEMO_REPO, 'dep-updater', rDeps, tNode, wsNode.head, {
    model: 'example-coder-small',
    rationale: 'Node 20 is in maintenance until April; the workflow and engines field now say 22.',
    checks: [],
    inputs: ['https://nodejs.org/en/about/previous-releases'],
  })

  // ---- reviews that happen before anything lands -----------------------------------------------------------
  log('reviews')
  await review(bob, DEMO_REPO, pGreeting.id, 'approve', 'Reads well. Thanks.')
  await review(
    bob,
    DEMO_REPO,
    pDocs.id,
    'approve',
    'Accurate, and the example matches what the middleware reads.',
  )
  await review(
    bob,
    DEMO_REPO,
    pRounding.id,
    'approve',
    'Rounding per line is what the printed invoice shows.',
  )
  await review(
    bob,
    DEMO_REPO,
    pFlaky.id,
    'request_changes',
    'The literal is good, but please add a regression test for a fractional quantity before this lands.',
  )
  await review(
    bob,
    DEMO_REPO,
    pNode.id,
    'approve',
    'Node 22 is fine. The workflow is protected, so someone else must approve too.',
  )

  // Alice pushes again after bob approved: his approval no longer covers the head.
  await amend(
    alice,
    DEMO_REPO,
    wsRounding.workspace,
    { 'tests/invoice.test.ts': ROUNDING['tests/invoice.test.ts']! },
    'Add a test for fractional quantities',
  )
  const newHead = (await must(alice.get(`${R}/workspaces/${wsRounding.workspace}`), 'workspace'))
    .head as string
  await must(
    alice.patch(`${R}/proposals/${pRounding.id}`, { new_head: newHead }),
    'update proposal head',
  )

  // ---- the one that lands first: friendly greeting ------------------------------------------------------------
  log('landing the greeting')
  await validated(alice, DEMO_REPO, pGreeting.id)
  await integrate(carol, DEMO_REPO, pGreeting.id)

  // ---- validations on the final stream (candidates changed when the greeting landed) -------------------------------
  log('validations')
  // The CI stand-in fails the flaky-test proposal on purpose: a trusted FAIL denies that exact tree.
  failChecksFor(pFlaky.id)
  for (const id of [pExtract.id, pDocs.id, pFlaky.id, pNode.id])
    await validated(alice, DEMO_REPO, id)

  // The docs change is complete (approved + validated): carol queues it. The queue stays queued.
  await must(carol.post(`${R}/queue`, { proposal: pDocs.id }), 'enqueue docs')

  // dep-updater's change has one of the two approvals its protected path needs; sanity-check the verdicts.
  await waitFor('the verdicts to settle', async () => {
    const p = await prepare(carol, DEMO_REPO, pExtract.id)
    return p.validations?.[0]?.state === 'passed'
  })

  return {
    repo: DEMO_REPO,
    proposals: {
      extract: pExtract.id,
      docs: pDocs.id,
      rounding: pRounding.id,
      flaky: pFlaky.id,
      formal: pFormal.id,
      node: pNode.id,
      greeting: pGreeting.id,
    },
    tasks: {
      import: tImport,
      extract: tExtract,
      docs: tDocs,
      flaky: tFlaky,
      node: tNode,
      redis: tRedis,
      metrics: tMetrics,
      runbook: tRunbook,
    },
    runs,
    leases: {
      auth: lAuth.lease.id,
      billing: lBilling.lease.id,
      docs: lDocs.lease.id,
      ci: lCi.lease.id,
      exploring: lLook.lease.id,
    },
    fencing: { auth: lAuth.lease.fencing_token, billing: lBilling.lease.fencing_token },
    headBeforeStory,
    pin,
  }
}

/** A repository with nothing in it: the empty states. */
export async function seedEmpty(cast: Cast): Promise<void> {
  await must(
    cast
      .as('admin')
      .post('/v1/repos', { name: EMPTY_REPO, description: 'Placeholder for the mobile client.' }),
    'create empty repo',
  )
}

/**
 * A repository where every piece of text a server can send is hostile markup. The UI must show it as
 * text, never run it, never lay it out as elements.
 */
export const HOSTILE = {
  description: `<img src=x onerror="window.__pwned='description'"> lab`,
  proposalTitle: `"><script>window.__pwned='title'</script> Fix <b>bold</b>`,
  proposalBody: `<svg onload="window.__pwned='body'"></svg>\n[click](javascript:window.__pwned='link')`,
  taskTitle: `<iframe srcdoc="<script>parent.__pwned='task'</script>"></iframe>`,
  review: `<img src=x onerror="window.__pwned='review'">`,
  file: `docs/<b>x</b>.md`,
  fileText: `<script>window.__pwned='file'</script>\n<a href="javascript:window.__pwned='a'">x</a>\n`,
  runSummary: `<style>body{display:none}</style>done`,
}

export async function seedHostile(cast: Cast): Promise<{ proposal: string }> {
  const R = `/v1/repos/${HOSTILE_REPO}`
  const admin = cast.as('admin')
  const alice = cast.as('alice')
  const bob = cast.as('bob')
  const carol = cast.as('carol')
  await must(
    admin.post('/v1/repos', { name: HOSTILE_REPO, description: HOSTILE.description }),
    'create hostile repo',
  )
  const base = await commitFiles(
    alice,
    HOSTILE_REPO,
    { 'index.html': HOSTILE.fileText, 'README.md': HOSTILE.proposalBody },
    `Import <u>underlined</u> things`,
  )
  const p0 = await openProposal(alice, HOSTILE_REPO, { head: base.head, title: 'Import' })
  await integrate(carol, HOSTILE_REPO, p0.id)
  const next = await commitFiles(
    alice,
    HOSTILE_REPO,
    { 'index.html': HOSTILE.fileText + '<!-- changed -->\n', [HOSTILE.file]: HOSTILE.fileText },
    HOSTILE.proposalTitle,
  )
  const p = await openProposal(alice, HOSTILE_REPO, {
    head: next.head,
    title: HOSTILE.proposalTitle,
    description: HOSTILE.proposalBody,
  })
  await review(bob, HOSTILE_REPO, p.id, 'comment', HOSTILE.review)
  await must(
    alice.post(`${R}/tasks`, { title: HOSTILE.taskTitle, description: HOSTILE.proposalBody }),
    'hostile task',
  )
  await must(
    cast
      .as('docs-writer')
      .post(`${R}/agents`, { name: 'docs-writer', public_key: agentKey('docs-writer').publicHex }),
    'register in hostile repo',
  )
  const run = await must(
    cast.as('docs-writer').post(`${R}/runs`, { agent: 'docs-writer' }),
    'hostile run',
  )
  await must(
    cast.as('docs-writer').post(`${R}/runs/${run.run_id}/complete`, {
      status: 'completed',
      summary: HOSTILE.runSummary,
    }),
    'finish hostile run',
  )
  return { proposal: p.id }
}
