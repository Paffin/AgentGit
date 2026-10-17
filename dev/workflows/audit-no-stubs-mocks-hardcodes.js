export const meta = {
  name: 'audit-no-stubs-mocks-hardcodes',
  description: 'Read-only audit of the whole repo for mocks, stubs, fakes, simulated behaviour, hardcoded values and unbuilt claims, with adversarial verification',
  whenToUse: 'Before declaring AgentGit production-ready: find every place where something is faked, hardcoded or only claimed',
  phases: [
    { title: 'Scan', detail: 'one reader per area lists candidate findings with evidence' },
    { title: 'Verify', detail: 'two skeptics per area (reachability lens, legitimacy lens) try to refute each finding' },
    { title: 'Coverage', detail: 'a critic compares files actually read with the files that exist' },
  ],
}

const ROOT = '/home/user/AgentGit'

const AREAS = [
  { key: 'core-formats', paths: 'crates/ag-types, crates/ag-object, crates/ag-changeset, crates/ag-crypto' },
  { key: 'core-merge', paths: 'crates/ag-tree, crates/ag-dag, crates/ag-diff, crates/ag-merge' },
  { key: 'policy-agent-protocol', paths: 'crates/ag-policy, crates/ag-agent, crates/ag-protocol, proto/' },
  { key: 'engine', paths: 'crates/ag-integrate' },
  { key: 'storage', paths: 'crates/ag-storage, migrations/' },
  { key: 'server-security', paths: 'crates/ag-server/src (auth, authz, security, config, rate limiting, quotas, tls, main, server, app, repos)' },
  { key: 'server-api', paths: 'crates/ag-server/src/api, http, grpc/routes, events/SSE, jobs, metrics, trace' },
  { key: 'cli-client-git', paths: 'crates/ag-cli, crates/ag-client, crates/ag-git' },
  { key: 'execution-plane', paths: 'crates/ag-worker, crates/ag-sandbox' },
  { key: 'web-ui', paths: 'apps/web/src, apps/web/vite.config.*, apps/web/index.html (NOT e2e/ or *.test.ts)' },
  { key: 'orchestrator', paths: 'services/agent-orchestrator (including internal/testutil — judge it as a test double)' },
  { key: 'sdks', paths: 'sdk/typescript, sdk/python (generators and hand-written parts)' },
  { key: 'ops-docs-claims', paths: 'deploy/, scripts/, .github/workflows, examples/, docs/ — and cross-check README.md feature claims against what the code really does' },
]

const FINDING = {
  type: 'object',
  properties: {
    file: { type: 'string' },
    line: { type: 'integer' },
    category: { type: 'string', enum: ['mock', 'stub', 'fake', 'simulated-behaviour', 'hardcoded-value', 'hardcoded-credential', 'hardcoded-url-or-host', 'placeholder-data', 'todo-or-temporary', 'heuristic-posing-as-real', 'test-double-where-real-is-feasible', 'claim-without-implementation'] },
    excerpt: { type: 'string' },
    problem: { type: 'string' },
    severity: { type: 'string', enum: ['high', 'medium', 'low'] },
    production_path: { type: 'boolean' },
    proposed_fix: { type: 'string' },
  },
  required: ['file', 'line', 'category', 'excerpt', 'problem', 'severity', 'production_path', 'proposed_fix'],
}
const SCAN = {
  type: 'object',
  properties: {
    area: { type: 'string' },
    files_read: { type: 'array', items: { type: 'string' } },
    findings: { type: 'array', items: FINDING },
  },
  required: ['area', 'files_read', 'findings'],
}
const VERDICTS = {
  type: 'object',
  properties: {
    verdicts: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          index: { type: 'integer' },
          real: { type: 'boolean' },
          reason: { type: 'string' },
          refined_fix: { type: 'string' },
        },
        required: ['index', 'real', 'reason'],
      },
    },
  },
  required: ['verdicts'],
}
const COVERAGE = {
  type: 'object',
  properties: {
    unread_source_files: { type: 'array', items: { type: 'string' } },
    notes: { type: 'string' },
  },
  required: ['unread_source_files', 'notes'],
}

const SCAN_BRIEF = `You are auditing the AgentGit repository at ${ROOT} (read-only: do NOT modify, build or run anything heavy; use Read/Grep/Glob). The owner's requirement: the product must contain NO mocks, NO stubs, NO fakes, NO simulated behaviour, NO hardcoded values that should be configuration or derived from real state, and no claims in docs/README that the code does not back.

READ EVERY SOURCE FILE in your area (not only grep hits — skim fully, tests excluded except where stated). Report CANDIDATES with exact file:line and a verbatim excerpt. What counts:
 - mock / stub / fake / dummy implementations or branches that return canned data; simulated behaviour (sleeps standing in for work, random/placeholder results, fake progress); "for now", temporary, best-effort-that-silently-does-nothing code; functions that accept arguments and ignore them; swallowed errors that hide missing functionality; empty or always-true/false implementations.
 - hardcoded values that should come from configuration or real state: URLs/hosts/ports without a config override, credentials/tokens/keys/emails/names, fixed timeouts/limits/sizes/intervals that an operator would need to tune and cannot, hardcoded user-facing demo data, hardcoded versions, magic numbers without a named, documented constant, fixed file system paths, environment assumptions.
 - heuristics presented as intelligence (e.g. an "advisor" or "reviewer" that is keyword matching); test doubles used where the real component is feasible (judge: is the real server/engine/service available to tests? then a fake is a defect).
 - claims in README/docs/UI text/CLI help of features that are not (fully) implemented.
 Do NOT report: constants mandated by a format/protocol/spec (hash domain separators, magic bytes, version tags, algorithm parameters that are part of a versioned, documented algorithm), documented defaults that ARE overridable via config/flags/env, values inside test files, ordinary UI placeholder= attributes and i18n strings, enum/test fixtures under tests/ or e2e/ seed data (but DO report a fake used in the product code path). If something is borderline, report it with severity low and say why.
 Be concrete and exhaustive but precise: each finding needs the line, what is faked/hardcoded, why it is a problem for a production product, and the REAL replacement (what to build, how to verify it with a real component). Return files_read listing every file you actually opened. If your area is clean, return an empty findings array — do not invent findings.`

const results = await pipeline(
  AREAS,
  (area) => agent(
    `${SCAN_BRIEF}\n\nYOUR AREA: "${area.key}" — ${area.paths}`,
    { label: `scan:${area.key}`, phase: 'Scan', schema: SCAN },
  ),
  async (scan, area) => {
    if (!scan || !scan.findings || scan.findings.length === 0) {
      return { area: area.key, scan, findings: [], verdicts: [] }
    }
    const list = scan.findings.map((f, i) => `[${i}] ${f.file}:${f.line} (${f.category}, ${f.severity}) — ${f.problem}\n    excerpt: ${f.excerpt}\n    proposed fix: ${f.proposed_fix}`).join('\n')
    const lenses = [
      { key: 'reachability', brief: 'REACHABILITY lens: for each finding open the file and trace the callers. Is this code on a production path (reachable from a binary entry point, server route, CLI command, UI view) or only from tests/examples? Is the value actually used in production? Mark real=false if it is test-only, dead code, or not reachable.' },
      { key: 'legitimacy', brief: 'LEGITIMACY lens: for each finding decide whether it is a genuine defect or legitimate: a format/protocol/spec constant, an overridable documented default, a named documented limit that is configurable, a deliberate deterministic algorithm (not posing as intelligence), or an accurate documented limitation. Mark real=true only if an operator/user would be genuinely hurt or misled and the fix is warranted. Check config files and docs for an existing override before deciding.' },
    ]
    const votes = await parallel(lenses.map(l => () => agent(
      `You are an adversarial verifier on the AgentGit repo at ${ROOT} (read-only). Another auditor reported candidate findings for area "${area.key}". Try to REFUTE each one — default to real=false when the evidence does not hold up in the actual code (open the files; do not trust the excerpt).\n${l.brief}\nFor every finding index return {index, real, reason (one or two sentences citing what you read), refined_fix (only if real; a precise, buildable replacement)}.\n\nFINDINGS:\n${list}`,
      { label: `verify:${area.key}:${l.key}`, phase: 'Verify', schema: VERDICTS },
    )))
    return { area: area.key, scan, findings: scan.findings, votes: votes.filter(Boolean).map(v => v.verdicts) }
  },
)

phase('Coverage')
const readByArea = results.filter(Boolean).map(r => ({ area: r.area, files: (r.scan && r.scan.files_read) || [] }))
const coverage = await agent(
  `Audit-coverage critic for AgentGit at ${ROOT} (read-only). Scanners reported which files they actually read, per area:\n${JSON.stringify(readByArea)}\n\nUsing Glob, list the production source files that exist (Rust: crates/*/src/**/*.rs and crates/*/build.rs; proto/**/*.proto; Vue/TS: apps/web/src/**/*.{vue,ts}; Go: services/agent-orchestrator/**/*.go excluding _test.go; sdk/typescript/src and sdk/python source; scripts/**; deploy/**; .github/workflows/*; examples/*; migrations/**) and return the ones NOT in any scanner's files_read (exclude tests, generated lockfiles, node_modules, target, vendored code, images). Put judgement about notable blind spots in notes.`,
  { label: 'coverage-critic', phase: 'Coverage', schema: COVERAGE },
)

const report = []
for (const r of results.filter(Boolean)) {
  r.findings.forEach((f, i) => {
    const vs = (r.votes || []).map(v => (v || []).find(x => x.index === i)).filter(Boolean)
    const yes = vs.filter(v => v.real).length
    report.push({ area: r.area, ...f, votes_real: yes, votes_total: vs.length, reasons: vs.map(v => v.reason), refined_fix: (vs.find(v => v.real && v.refined_fix) || {}).refined_fix || f.proposed_fix, status: vs.length === 0 ? 'unverified' : yes === vs.length ? 'confirmed' : yes === 0 ? 'refuted' : 'contested' })
  })
}
const counts = { confirmed: 0, contested: 0, refuted: 0, unverified: 0 }
report.forEach(f => counts[f.status]++)
log(`findings: ${JSON.stringify(counts)}; areas clean: ${results.filter(Boolean).filter(r => r.findings.length === 0).map(r => r.area).join(', ') || 'none'}; unread files: ${coverage ? coverage.unread_source_files.length : '?'}`)
return { counts, report: report.filter(f => f.status !== 'refuted'), refuted_count: counts.refuted, coverage }
