// The rules of a branch (the policy document) in plain words: presets to start from, a form model that reads
// and writes the same JSON the server stores, sentences that say what the rules allow and deny, and a change
// described as what it now allows or denies. Advice only: the server parses the document strictly and decides.
// Fields this file does not know are carried through untouched, so editing the form never drops what the raw
// JSON editor (Advanced) holds.

export type PolicyDoc = Record<string, unknown>

export type PresetId = 'solo' | 'team' | 'agents' | 'strict'

export interface Preset {
  id: PresetId
  name: string
  /** Who it is for, in one sentence. */
  forWhom: string
  /** What it does, in plain words (the document's own description is generated from the JSON). */
  summary: string
  doc: PolicyDoc
}

/** Fields that belong to the people running CI, not to a preset: a preset keeps whatever is already there. */
const KEEP = ['trusted_issuers', 'required_validations', 'integrators', 'admins', 'allowed_authors']

export const PRESETS: readonly Preset[] = [
  {
    id: 'solo',
    name: 'Solo',
    forWhom: 'One person, maybe with agents. You keep the history safe and approve your own work.',
    summary:
      'No review is needed for your own changes. An agent’s change still needs one approval, and you may give it yourself.',
    doc: { approvals: { min: 0, min_for_agents: 1, forbid_self_approval: false } },
  },
  {
    id: 'team',
    name: 'Team',
    forWhom: 'A few people who review each other’s work.',
    summary:
      'Every change needs one approval from someone other than its author. Agents cannot land changes by themselves.',
    doc: { approvals: { min: 1, min_for_agents: 1, forbid_self_approval: true } },
  },
  {
    id: 'agents',
    name: 'Agents with guardrails',
    forWhom: 'Agents do routine work; people stay in charge of the rest.',
    summary:
      'Agents may land small changes to docs and tests by themselves, if they sign where the change came from. Anything else, and every change under .github, needs people to approve it.',
    doc: {
      approvals: { min: 1, min_for_agents: 1, forbid_self_approval: true },
      agent_autonomy: {
        auto_integrate: true,
        allowed_paths: ['docs/**', 'tests/**'],
        max_files: 10,
      },
      require_provenance_for_agents: true,
      protected_paths: [{ glob: '.github/**', min_approvals: 2 }],
    },
  },
  {
    id: 'strict',
    name: 'Strict',
    forWhom: 'Sensitive code. Two pairs of eyes on everything.',
    summary:
      'Two approvals for every change, agents included, with a signed record of where an agent’s change came from. Changes to .github and SECURITY.md need two approvals as well. A change may touch at most 200 files.',
    doc: {
      approvals: { min: 2, min_for_agents: 2, forbid_self_approval: true },
      require_provenance_for_agents: true,
      protected_paths: [
        { glob: '.github/**', min_approvals: 2 },
        { glob: 'SECURITY.md', min_approvals: 2 },
      ],
      max_changed_files: 200,
    },
  },
]

export const presetById = (id: PresetId): Preset => PRESETS.find((p) => p.id === id) as Preset

/** The document of a preset, keeping who-may and check settings that the branch already has. */
export function applyPreset(id: PresetId, base: PolicyDoc = {}): PolicyDoc {
  const out: PolicyDoc = JSON.parse(JSON.stringify(presetById(id).doc)) as PolicyDoc
  for (const k of KEEP) if (base[k] !== undefined) out[k] = base[k]
  return out
}

export function parseDoc(text: string): PolicyDoc | null {
  try {
    const v: unknown = JSON.parse(text)
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as PolicyDoc) : null
  } catch {
    return null
  }
}

export function stringifyDoc(doc: PolicyDoc): string {
  return JSON.stringify(doc, null, 2) + '\n'
}

/** Which preset a document is, if it is exactly one (ignoring the settings a preset keeps). */
export function detectPreset(doc: PolicyDoc): PresetId | null {
  const strip = (d: PolicyDoc) => {
    const c: PolicyDoc = { ...d }
    for (const k of KEEP) delete c[k]
    return JSON.stringify(sortKeys(c))
  }
  const mine = strip(doc)
  return PRESETS.find((p) => strip(p.doc) === mine)?.id ?? null
}

function sortKeys(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(sortKeys)
  if (v && typeof v === 'object')
    return Object.fromEntries(
      Object.entries(v as Record<string, unknown>)
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([k, x]) => [k, sortKeys(x)]),
    )
  return v
}

// ---- the form model ------------------------------------------------------------------------------------

export interface PolicyForm {
  approvalsMin: number
  approvalsMinForAgents: number
  forbidSelfApproval: boolean
  agentsMayLand: boolean
  agentPaths: string
  agentMaxFiles: number
  requireProvenance: boolean
  protectedPaths: { glob: string; minApprovals: number }[]
  maxChangedFiles: number
  /** Names of required checks (read-only here: adding one needs a trusted runner, which is under Advanced). */
  checks: string[]
}

const num = (v: unknown, d = 0): number => (typeof v === 'number' && Number.isFinite(v) ? v : d)
const obj = (v: unknown): Record<string, unknown> =>
  v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
const strs = (v: unknown): string[] =>
  Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []

export function formFromDoc(doc: PolicyDoc): PolicyForm {
  const a = obj(doc.approvals)
  const au = obj(doc.agent_autonomy)
  return {
    approvalsMin: num(a.min),
    // The server's own default when the field is absent.
    approvalsMinForAgents: num(a.min_for_agents, 1),
    // The server's default when the field is absent is to ignore an author's own approval.
    forbidSelfApproval: a.forbid_self_approval !== false,
    agentsMayLand: au.auto_integrate === true,
    agentPaths: strs(au.allowed_paths).join(', '),
    agentMaxFiles: num(au.max_files),
    requireProvenance: doc.require_provenance_for_agents === true,
    protectedPaths: (Array.isArray(doc.protected_paths) ? doc.protected_paths : []).map((p) => ({
      glob: String(obj(p).glob ?? ''),
      minApprovals: num(obj(p).min_approvals, 1),
    })),
    maxChangedFiles: num(doc.max_changed_files),
    checks: (Array.isArray(doc.required_validations) ? doc.required_validations : [])
      .map((v) => String(obj(v).check ?? ''))
      .filter(Boolean),
  }
}

/** The document for a form, written over `base` so that everything the form does not show survives. */
export function docFromForm(form: PolicyForm, base: PolicyDoc = {}): PolicyDoc {
  const out: PolicyDoc = { ...base }
  const approvals = { ...obj(base.approvals) }
  approvals.min = form.approvalsMin
  approvals.min_for_agents = form.approvalsMinForAgents
  approvals.forbid_self_approval = form.forbidSelfApproval
  out.approvals = approvals
  const paths = form.agentPaths
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter(Boolean)
  if (form.agentsMayLand) {
    out.agent_autonomy = {
      ...obj(base.agent_autonomy),
      auto_integrate: true,
      allowed_paths: paths,
      max_files: form.agentMaxFiles,
    }
  } else {
    delete out.agent_autonomy
  }
  if (form.requireProvenance) out.require_provenance_for_agents = true
  else delete out.require_provenance_for_agents
  const protectedPaths = form.protectedPaths
    .filter((p) => p.glob.trim())
    .map((p) => ({ glob: p.glob.trim(), min_approvals: p.minApprovals }))
  if (protectedPaths.length) {
    // Keep each rule's own approvers when the glob is unchanged.
    const old = Array.isArray(base.protected_paths) ? base.protected_paths.map(obj) : []
    out.protected_paths = protectedPaths.map((p) => {
      const was = old.find((o) => o.glob === p.glob)
      return was ? { ...was, ...p } : p
    })
  } else delete out.protected_paths
  if (form.maxChangedFiles > 0) out.max_changed_files = form.maxChangedFiles
  else delete out.max_changed_files
  return out
}

// ---- plain sentences -----------------------------------------------------------------------------------

const list = (xs: string[]): string =>
  xs.length <= 1 ? (xs[0] ?? '') : `${xs.slice(0, -1).join(', ')} and ${xs[xs.length - 1]}`
const approvalsWord = (n: number) => `${n} ${n === 1 ? 'approval' : 'approvals'}`

/** One sentence per setting that matters, in the order a person asks the questions. */
export function describePolicy(doc: PolicyDoc): string[] {
  const out: string[] = []
  const a = obj(doc.approvals)
  const min = num(a.min)
  const forAgents = num(a.min_for_agents, 1)
  const integrators = strs(doc.integrators)
  const authors = strs(doc.allowed_authors)
  out.push(
    integrators.length
      ? `Only ${list(integrators)} may land changes.`
      : 'Any person may land a change (agents only within their limits).',
  )
  out.push(
    authors.length ? `Only ${list(authors)} may propose changes.` : 'Anyone may propose changes.',
  )
  out.push(
    min === 0
      ? 'A person’s change needs no approval.'
      : `A change needs ${approvalsWord(min)} of its latest version${a.forbid_self_approval !== false ? ', from someone other than its author' : ''}.`,
  )
  if (a.forbid_self_approval === false && min > 0)
    out.push('Authors may approve their own changes.')
  out.push(
    `An agent’s change needs ${approvalsWord(Math.max(min, forAgents))} from people${forAgents === 0 ? ' (none required)' : ''} unless it stays within the limits below.`,
  )
  const au = obj(doc.agent_autonomy)
  const paths = strs(au.allowed_paths)
  if (au.auto_integrate === true && paths.length && num(au.max_files) > 0)
    out.push(
      `Agents may land a change by themselves if it touches only ${list(paths)} and at most ${num(au.max_files)} files.`,
    )
  else out.push('Agents cannot land changes by themselves.')
  if (doc.require_provenance_for_agents === true)
    out.push('An agent’s change must carry a signed record of where it came from.')
  const checks = (Array.isArray(doc.required_validations) ? doc.required_validations : [])
    .map((v) => String(obj(v).check ?? ''))
    .filter(Boolean)
  out.push(
    checks.length
      ? `These checks must pass on the exact files before landing: ${list(checks)}.`
      : 'No checks are required before landing.',
  )
  for (const p of Array.isArray(doc.protected_paths) ? doc.protected_paths.map(obj) : [])
    out.push(`Changes to ${String(p.glob)} need ${approvalsWord(num(p.min_approvals, 1))}.`)
  if (num(doc.max_changed_files) > 0)
    out.push(`A change may touch at most ${num(doc.max_changed_files)} files.`)
  const admins = strs(doc.admins)
  out.push(
    admins.length
      ? `Only ${list(admins)} may change these rules.`
      : 'Any person may change these rules.',
  )
  return out
}

export interface Effect {
  /** `allows`: something becomes possible or easier; `denies`: something is no longer possible or harder. */
  kind: 'allows' | 'denies' | 'changes'
  text: string
}

/** What changing the rules from `before` to `after` does, as effects on people and agents. */
export function describeChange(before: PolicyDoc, after: PolicyDoc): Effect[] {
  const out: Effect[] = []
  const a = obj(before.approvals)
  const b = obj(after.approvals)
  const num2 = (x: unknown, y: unknown, d = 0): [number, number] => [num(x, d), num(y, d)]

  const [min0, min1] = num2(a.min, b.min)
  if (min1 > min0)
    out.push({ kind: 'denies', text: `Landing now needs ${approvalsWord(min1)} (was ${min0}).` })
  else if (min1 < min0)
    out.push({
      kind: 'allows',
      text: `Landing now needs ${min1 === 0 ? 'no approval' : approvalsWord(min1)} (was ${min0}).`,
    })
  const [ag0, ag1] = num2(a.min_for_agents, b.min_for_agents, 1)
  if (ag1 > ag0)
    out.push({
      kind: 'denies',
      text: `An agent’s change now needs ${approvalsWord(ag1)} from people (was ${ag0}).`,
    })
  else if (ag1 < ag0)
    out.push({
      kind: 'allows',
      text: `An agent’s change now needs only ${approvalsWord(ag1)} from people (was ${ag0}).`,
    })
  const self0 = a.forbid_self_approval !== false
  const self1 = b.forbid_self_approval !== false
  if (self1 && !self0)
    out.push({ kind: 'denies', text: 'Authors can no longer approve their own changes.' })
  if (!self1 && self0)
    out.push({ kind: 'allows', text: 'Authors may now approve their own changes.' })

  const au0 = obj(before.agent_autonomy)
  const au1 = obj(after.agent_autonomy)
  const on0 = au0.auto_integrate === true
  const on1 = au1.auto_integrate === true
  if (on1 && !on0)
    out.push({
      kind: 'allows',
      text: `Agents may now land changes by themselves when they touch only ${list(strs(au1.allowed_paths))} and at most ${num(au1.max_files)} files.`,
    })
  else if (!on1 && on0)
    out.push({ kind: 'denies', text: 'Agents can no longer land changes by themselves.' })
  else if (
    on1 &&
    (JSON.stringify(au0.allowed_paths) !== JSON.stringify(au1.allowed_paths) ||
      num(au0.max_files) !== num(au1.max_files))
  )
    out.push({
      kind: 'changes',
      text: `Agents may land by themselves only for ${list(strs(au1.allowed_paths))}, at most ${num(au1.max_files)} files (was ${list(strs(au0.allowed_paths))}, ${num(au0.max_files)}).`,
    })

  const pv0 = before.require_provenance_for_agents === true
  const pv1 = after.require_provenance_for_agents === true
  if (pv1 && !pv0)
    out.push({
      kind: 'denies',
      text: 'Agent changes must now carry a signed record of where they came from.',
    })
  if (!pv1 && pv0)
    out.push({
      kind: 'allows',
      text: 'Agent changes no longer need a signed record of where they came from.',
    })

  const checks = (d: PolicyDoc) =>
    (Array.isArray(d.required_validations) ? d.required_validations : [])
      .map((v) => String(obj(v).check ?? ''))
      .filter(Boolean)
  const c0 = checks(before)
  const c1 = checks(after)
  const added = c1.filter((c) => !c0.includes(c))
  const removed = c0.filter((c) => !c1.includes(c))
  if (added.length)
    out.push({
      kind: 'denies',
      text: `Landing now also needs these checks to pass: ${list(added)}.`,
    })
  if (removed.length)
    out.push({ kind: 'allows', text: `These checks are no longer required: ${list(removed)}.` })

  const pp = (d: PolicyDoc) =>
    new Map(
      (Array.isArray(d.protected_paths) ? d.protected_paths : []).map((p) => [
        String(obj(p).glob),
        num(obj(p).min_approvals, 1),
      ]),
    )
  const p0 = pp(before)
  const p1 = pp(after)
  for (const [g, n] of p1) {
    const was = p0.get(g)
    if (was === undefined)
      out.push({ kind: 'denies', text: `Changes to ${g} now need ${approvalsWord(n)}.` })
    else if (n > was)
      out.push({
        kind: 'denies',
        text: `Changes to ${g} now need ${approvalsWord(n)} (was ${was}).`,
      })
    else if (n < was)
      out.push({
        kind: 'allows',
        text: `Changes to ${g} now need only ${approvalsWord(n)} (was ${was}).`,
      })
  }
  for (const g of p0.keys())
    if (!p1.has(g))
      out.push({ kind: 'allows', text: `Changes to ${g} are no longer specially protected.` })

  const m0 = num(before.max_changed_files)
  const m1 = num(after.max_changed_files)
  if (m1 !== m0) {
    if (m1 > 0 && (m0 === 0 || m1 < m0))
      out.push({
        kind: 'denies',
        text: `A change may now touch at most ${m1} files${m0 ? ` (was ${m0})` : ''}.`,
      })
    else
      out.push({
        kind: 'allows',
        text:
          m1 === 0
            ? 'There is no longer a limit on how many files a change may touch.'
            : `A change may now touch up to ${m1} files (was ${m0}).`,
      })
  }

  for (const [key, who] of [
    ['integrators', 'land changes'],
    ['allowed_authors', 'propose changes'],
    ['admins', 'change these rules'],
  ] as const) {
    const x = strs(before[key])
    const y = strs(after[key])
    if (JSON.stringify(x) !== JSON.stringify(y))
      out.push({
        kind: 'changes',
        text: y.length
          ? `Who may ${who}: ${list(y)}.`
          : `Anyone allowed by default may now ${who.replace('change these rules', 'change these rules')}.`,
      })
  }
  return out
}
