// Client-side structure hints for the policy editor (docs/guides/policy.md). They are advice, not the
// authority: the server parses strictly and rejects unknown fields, and its message is always shown too.
import { parseJsonWithPositions } from './jsonpos'
import type { JsonValue, Pos } from './jsonpos'

export type IssueLevel = 'error' | 'warning' | 'hint'

export interface PolicyIssue {
  level: IssueLevel
  path: string
  message: string
  line: number | null
  col: number | null
}

type Kind = 'strings' | 'bool' | 'uint' | 'string' | 'object' | 'objects' | 'stringmap'

interface FieldSpec {
  kind: Kind
  fields?: Record<string, FieldSpec>
  note?: string
}

const RUN: Record<string, FieldSpec> = {
  argv: { kind: 'strings' },
  timeout_secs: { kind: 'uint' },
  memory_mb: { kind: 'uint' },
  cpu_secs: { kind: 'uint' },
  max_pids: { kind: 'uint' },
  network: { kind: 'bool' },
  image: { kind: 'string' },
  env: { kind: 'stringmap' },
}

export const POLICY_SPEC: Record<string, FieldSpec> = {
  require_resolved_threads: {
    kind: 'bool',
    note: 'all line discussions must be resolved before landing',
  },
  integrators: { kind: 'strings', note: 'who may integrate; empty = any non-agent' },
  admins: { kind: 'strings', note: 'who may change the policy; empty = any user' },
  allowed_authors: { kind: 'strings', note: 'who may author proposals; empty = anyone' },
  trusted_issuers: {
    kind: 'objects',
    fields: { identity: { kind: 'string' }, public_key: { kind: 'string' } },
  },
  required_validations: {
    kind: 'objects',
    fields: {
      check: { kind: 'string' },
      issuers: { kind: 'strings' },
      environment_digest: { kind: 'string' },
      max_age_secs: { kind: 'uint' },
      run: { kind: 'object', fields: RUN },
    },
  },
  approvals: {
    kind: 'object',
    fields: {
      min: { kind: 'uint' },
      min_for_agents: { kind: 'uint' },
      approvers: { kind: 'strings' },
      forbid_self_approval: { kind: 'bool' },
    },
  },
  protected_paths: {
    kind: 'objects',
    fields: {
      glob: { kind: 'string' },
      min_approvals: { kind: 'uint' },
      approvers: { kind: 'strings' },
    },
  },
  agent_autonomy: {
    kind: 'object',
    fields: {
      auto_integrate: { kind: 'bool' },
      allowed_paths: { kind: 'strings' },
      max_files: { kind: 'uint' },
    },
  },
  require_provenance_for_agents: { kind: 'bool' },
  max_changed_files: { kind: 'uint' },
}

export const POLICY_STARTER = `{
  "approvals": {
    "min": 1,
    "forbid_self_approval": true
  }
}
`

export function levenshtein(a: string, b: string): number {
  const prev = Array.from({ length: b.length + 1 }, (_, j) => j)
  for (let i = 1; i <= a.length; i++) {
    let diag = prev[0] ?? 0
    prev[0] = i
    for (let j = 1; j <= b.length; j++) {
      const up = prev[j] ?? 0
      prev[j] = Math.min(up + 1, (prev[j - 1] ?? 0) + 1, diag + (a[i - 1] === b[j - 1] ? 0 : 1))
      diag = up
    }
  }
  return prev[b.length] ?? 0
}

export function suggestKey(key: string, candidates: readonly string[]): string | null {
  let best: string | null = null
  let bestD = 3
  for (const c of candidates) {
    const d = levenshtein(key.toLowerCase(), c)
    if (d < bestD) {
      best = c
      bestD = d
    }
  }
  return best
}

const isObj = (v: JsonValue | undefined): v is { [k: string]: JsonValue } =>
  typeof v === 'object' && v !== null && !Array.isArray(v)

export interface AnalyzeOptions {
  /** The caller's identity, to warn about locking oneself out via `admins`. */
  me?: string
}

export function analyzePolicy(text: string, options: AnalyzeOptions = {}): PolicyIssue[] {
  const parsed = parseJsonWithPositions(text)
  const issues: PolicyIssue[] = []
  const at = (path: string): Pos | undefined => parsed.positions.get(path)
  const add = (level: IssueLevel, path: string, message: string) => {
    const p = at(path)
    issues.push({ level, path, message, line: p?.line ?? null, col: p?.col ?? null })
  }

  if (parsed.error) {
    issues.push({
      level: 'error',
      path: '',
      message: `Not valid JSON: ${parsed.error.message}`,
      line: parsed.error.line,
      col: parsed.error.col,
    })
    return issues
  }
  for (const d of parsed.duplicates)
    add('error', d.path, `Duplicate key “${d.path.split('.').pop()}”. The server rejects it.`)
  const doc = parsed.value
  if (!isObj(doc)) {
    issues.push({
      level: 'error',
      path: '',
      message: 'A policy is a JSON object.',
      line: 1,
      col: 1,
    })
    return issues
  }

  const checkKind = (path: string, v: JsonValue, spec: FieldSpec) => {
    const bad = (expected: string) => add('error', path, `Expected ${expected}.`)
    switch (spec.kind) {
      case 'bool':
        if (typeof v !== 'boolean') bad('true or false')
        break
      case 'uint':
        if (typeof v !== 'number' || !Number.isInteger(v) || v < 0)
          bad('a non-negative whole number')
        break
      case 'string':
        if (typeof v !== 'string') bad('a string')
        break
      case 'strings':
        if (!Array.isArray(v) || v.some((x) => typeof x !== 'string')) bad('a list of strings')
        break
      case 'stringmap':
        if (!isObj(v) || Object.values(v).some((x) => typeof x !== 'string'))
          bad('an object of strings')
        break
      case 'object':
        if (!isObj(v)) bad('an object')
        else checkFields(path, v, spec.fields ?? {})
        break
      case 'objects':
        if (!Array.isArray(v) || v.some((x) => !isObj(x))) bad('a list of objects')
        else
          v.forEach((item, idx) => {
            if (isObj(item)) checkFields(`${path}[${idx}]`, item, spec.fields ?? {})
          })
        break
    }
  }
  const checkFields = (
    base: string,
    obj: { [k: string]: JsonValue },
    fields: Record<string, FieldSpec>,
  ) => {
    const known = Object.keys(fields)
    for (const [key, v] of Object.entries(obj)) {
      const path = base ? `${base}.${key}` : key
      const spec = fields[key]
      if (!spec) {
        const hint = suggestKey(key, known)
        add(
          'error',
          path,
          `Unknown field “${key}”. The server rejects unknown fields${hint ? `; did you mean “${hint}”?` : ` (known here: ${known.join(', ')})`}.`,
        )
        continue
      }
      checkKind(path, v, spec)
    }
  }
  checkFields('', doc, POLICY_SPEC)
  if (issues.some((i) => i.level === 'error')) return issues

  // ---- semantic rules the server also enforces ----------------------------------------------------
  const issuers = Array.isArray(doc.trusted_issuers) ? doc.trusted_issuers : []
  const validations = Array.isArray(doc.required_validations) ? doc.required_validations : []
  if (validations.length > 0 && issuers.length === 0) {
    add(
      'error',
      'required_validations',
      'Required validations need at least one entry in trusted_issuers; the policy could never be satisfied.',
    )
  }
  issuers.forEach((it, idx) => {
    if (isObj(it)) {
      if (typeof it.public_key === 'string' && !/^[0-9a-f]{64}$/.test(it.public_key)) {
        add(
          'error',
          `trusted_issuers[${idx}].public_key`,
          'A public key is 64 lowercase hex characters (an Ed25519 key).',
        )
      }
      if (typeof it.identity === 'string' && !/^(service|user|agent):/.test(it.identity)) {
        add(
          'warning',
          `trusted_issuers[${idx}].identity`,
          'Identities look like service:ci, user:alice or agent:name.',
        )
      }
    }
  })
  validations.forEach((v, idx) => {
    if (isObj(v) && (typeof v.check !== 'string' || v.check === '')) {
      add(
        'error',
        `required_validations[${idx}].check`,
        'Every required validation names its check.',
      )
    }
  })
  const autonomy = isObj(doc.agent_autonomy) ? doc.agent_autonomy : null
  if (autonomy) {
    const paths = Array.isArray(autonomy.allowed_paths) ? autonomy.allowed_paths : []
    if (autonomy.auto_integrate === true && paths.length === 0) {
      add(
        'error',
        'agent_autonomy.auto_integrate',
        'auto_integrate needs a non-empty allowed_paths.',
      )
    }
    if (
      autonomy.auto_integrate === true &&
      !(typeof autonomy.max_files === 'number' && autonomy.max_files > 0)
    ) {
      add(
        'warning',
        'agent_autonomy.max_files',
        'With max_files at 0, nothing is ever inside the autonomy envelope.',
      )
    }
  }
  const approvals = isObj(doc.approvals) ? doc.approvals : null
  if (
    approvals &&
    typeof approvals.min_for_agents === 'number' &&
    typeof approvals.min === 'number' &&
    approvals.min_for_agents < approvals.min
  ) {
    add(
      'hint',
      'approvals.min_for_agents',
      'Agents outside the envelope need max(min, min_for_agents) approvals, so a value below min has no effect.',
    )
  }
  if (Array.isArray(doc.protected_paths)) {
    doc.protected_paths.forEach((p, idx) => {
      if (isObj(p) && (typeof p.glob !== 'string' || p.glob === ''))
        add(
          'error',
          `protected_paths[${idx}].glob`,
          'A protected path needs a glob such as “.github/**”.',
        )
      if (isObj(p) && typeof p.min_approvals === 'number' && p.min_approvals < 1)
        add(
          'warning',
          `protected_paths[${idx}].min_approvals`,
          'A protected path with no required approvals protects nothing.',
        )
    })
  }
  if (options.me && Array.isArray(doc.admins) && doc.admins.length > 0) {
    const me = options.me
    const principal = me.split('#')[0] ?? me
    const matches = doc.admins.some((p) => typeof p === 'string' && globMatch(p, principal))
    if (!matches)
      add(
        'warning',
        'admins',
        `You (${principal}) do not match admins. After saving you could not change this policy again.`,
      )
  }
  if (issues.length === 0) {
    issues.push({
      level: 'hint',
      path: '',
      message:
        'Looks well-formed. Saving creates a new policy version and invalidates attestations made under the old one.',
      line: null,
      col: null,
    })
  }
  return issues
}

function globMatch(pattern: string, value: string): boolean {
  const re = new RegExp(
    '^' +
      pattern
        .split('*')
        .map((s) => s.replace(/[.+^${}()|[\]\\?]/g, '\\$&'))
        .join('.*') +
      '$',
  )
  return re.test(value)
}

export function prettyPolicy(text: string): string | null {
  try {
    return JSON.stringify(JSON.parse(text), null, 2) + '\n'
  } catch {
    return null
  }
}

export interface PolicySummary {
  approvalsMin: number
  approvalsMinForAgents: number
  requiredChecks: string[]
  autoIntegrate: boolean
}

/** What the board needs from a policy document without re-implementing the evaluator. */
export function summarizePolicy(document: string): PolicySummary {
  const none: PolicySummary = {
    approvalsMin: 0,
    approvalsMinForAgents: 1,
    requiredChecks: [],
    autoIntegrate: false,
  }
  try {
    const d = JSON.parse(document) as Record<string, unknown>
    const approvals = (d.approvals ?? {}) as Record<string, unknown>
    const validations = Array.isArray(d.required_validations) ? d.required_validations : []
    const autonomy = (d.agent_autonomy ?? {}) as Record<string, unknown>
    return {
      approvalsMin: typeof approvals.min === 'number' ? approvals.min : 0,
      approvalsMinForAgents:
        typeof approvals.min_for_agents === 'number' ? approvals.min_for_agents : 1,
      requiredChecks: validations
        .map((v) => String((v as Record<string, unknown>).check ?? ''))
        .filter(Boolean),
      autoIntegrate: autonomy.auto_integrate === true,
    }
  } catch {
    return none
  }
}
