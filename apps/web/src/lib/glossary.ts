// The one place that maps AgentGit's words to the plain words the Web UI shows. Screens ask for a term by id
// (`<Term id="lease" />`, `termLabel('lease')`); the AgentGit word lives in the tooltip. The same table is printed in
// docs/guides/web-ui.md ("Plain words"), and tests/glossary-docs.spec.ts fails if the two drift apart.
// The CLI, the protocol and the API keep the AgentGit words: this is a reading aid, not a rename.

export interface GlossaryEntry {
  /** The plain label shown on screen (singular). */
  label: string
  /** The plural; defaults to `label + 's'`. */
  plural?: string
  /** What AgentGit calls it (shown in the tooltip, and the column of the docs table). */
  word: string
  /** One or two plain sentences. No jargon that is not itself a glossary entry. */
  tip: string
}

export const GLOSSARY = {
  proposal: {
    label: 'Change request',
    word: 'proposal',
    tip: 'A set of file changes waiting to be reviewed and landed.',
  },
  integrate: {
    label: 'Land',
    word: 'integrate',
    tip: 'Move the change into the branch.',
  },
  integrated: {
    label: 'Landed',
    plural: 'Landed',
    word: 'integrated',
    tip: 'Has the change been moved into the branch?',
  },
  stream: {
    label: 'Branch',
    plural: 'Branches',
    word: 'stream',
    tip: 'A line of work that changes land on, like a Git branch.',
  },
  verdict: {
    label: 'Status',
    word: 'verdict (the four states)',
    tip: 'Four separate questions: does it merge cleanly, did the checks pass, is it approved, has it landed. One being true does not make another true.',
  },
  mergeable: {
    label: 'Merge',
    word: 'mergeable',
    tip: 'Does the change combine with the branch as it is now, without conflicts?',
  },
  validated: {
    label: 'Checks',
    plural: 'Checks',
    word: 'validated',
    tip: 'Has every check the rules require passed, with a trusted result for these exact files?',
  },
  approved: {
    label: 'Approval',
    word: 'approved',
    tip: 'Does the change have the approvals the rules ask for, from reviewers who looked at its latest version?',
  },
  validation: {
    label: 'Check',
    plural: 'Checks',
    word: 'validation',
    tip: 'A test or other check the rules require before a change may land.',
  },
  attestation: {
    label: 'Check result',
    word: 'attestation',
    tip: 'A signed result from a trusted runner for one check on one exact version of the files.',
  },
  provenance: {
    label: 'Where it came from',
    word: 'provenance',
    tip: 'What the agent says about its own change: model, reason and inputs, signed by the agent. Treat it as a claim, not a fact.',
  },
  lease: {
    label: 'Reservation',
    word: 'lease',
    tip: 'A temporary claim on some files so that two agents do not edit them at once.',
  },
  exclusiveLease: {
    label: 'Exclusive reservation',
    plural: 'Exclusive reservations',
    word: 'exclusive lease',
    tip: 'Nobody else may land changes to these files until it ends or is released.',
  },
  fencing: {
    label: 'Reservation key',
    word: 'fencing token',
    tip: 'A number that proves you hold the newest reservation. Landing a change under an exclusive reservation needs it.',
  },
  policy: {
    label: 'Rules',
    plural: 'Rules',
    word: 'policy',
    tip: 'Who may land changes to a branch, and which checks and approvals are needed first.',
  },
  changeset: {
    label: 'Version',
    word: 'changeset',
    tip: 'One saved state of the files.',
  },
  run: {
    label: 'Agent session',
    word: 'run',
    tip: 'One working session of an agent on a task.',
  },
  workspace: {
    label: 'Draft',
    word: 'workspace',
    tip: 'A private working copy of a branch, where edits collect before they are proposed.',
  },
  quiet: {
    label: 'Silent',
    plural: 'Silent',
    word: 'quiet (stale heartbeat)',
    tip: 'The agent has not reported in for a while. It may be stuck; this is a hint, not a verdict.',
  },
} as const satisfies Record<string, GlossaryEntry>

export type TermId = keyof typeof GLOSSARY

export const TERM_IDS = Object.keys(GLOSSARY) as TermId[]

const entry = (id: TermId): GlossaryEntry => GLOSSARY[id]

/** The plain label as a string, for places that are not a component (tab names, dialog titles). */
export function termLabel(id: TermId, plural = false): string {
  const e = entry(id)
  return plural ? (e.plural ?? `${e.label}s`) : e.label
}

/** The tooltip text: the plain meaning, then what AgentGit calls it. */
export function termTip(id: TermId): string {
  const e = entry(id)
  return `${e.tip} AgentGit calls this “${e.word}”.`
}

/** The glossary as the Markdown table printed in the guide (used by the docs test). */
export function glossaryTable(): string {
  const rows = TERM_IDS.map((id) => {
    const e = entry(id)
    return `| ${e.word} | ${e.label} | ${e.tip} |`
  })
  return ['| AgentGit word | Plain label | Meaning |', '| --- | --- | --- |', ...rows].join('\n')
}
