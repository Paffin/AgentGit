import { samePrincipal } from './identity'

// The commands of the "connect your first agent" panel. They are plain data built here (and not typed into a
// template) so that a test can take them from the page and run them against a real server: e2e/onboarding.spec.ts
// does exactly that. Nothing in them is invented: the endpoints are the server's (`POST /v1/roles`,
// `POST /v1/credentials`), the CLI commands are `ag`'s.

/** What the panel shows where the agent's secret goes; the person replaces it with the `token` of step 2. */
export const TOKEN_SLOT = '<token from step 2>'
/** The environment variable that holds an administrator's token in the commands of steps 1 and 2. */
export const ADMIN_TOKEN_VAR = 'AG_ADMIN_TOKEN'
/** How long the credential in step 2 lives. The server caps agent credentials; an hour is always allowed. */
export const CREDENTIAL_TTL_SECS = 3600

export interface ConnectStep {
  id: 'role' | 'credential' | 'work'
  title: string
  /** What the step does and what to expect, in a sentence or two. */
  note: string
  /** The shell lines, in order. */
  lines: string[]
  /** Needs an administrator (steps 1 and 2); the rest can be run by whoever holds the agent's token. */
  admin: boolean
}

export interface ConnectInput {
  /** The server's address as the browser sees it (`location.origin`). */
  origin: string
  /** `owner/name`. */
  repo: string
  /** The agent's identity, `agent:<name>`. */
  subject: string
}

/** Subjects the panel will put into a shell line: letters, digits and `._-@:` after a kind prefix. */
const SUBJECT = /^(agent|user|service):[A-Za-z0-9][A-Za-z0-9._@-]{0,62}$/
const REPO = /^[A-Za-z0-9][A-Za-z0-9._-]*\/[A-Za-z0-9][A-Za-z0-9._-]*$/

/** Why `subject` cannot be used in the commands, or `null` when it can. */
export function subjectProblem(subject: string): string | null {
  return SUBJECT.test(subject)
    ? null
    : 'Use agent:<name> (letters, digits, dot, dash, underscore), for example agent:my-agent.'
}

/** Single-quotes a value for a POSIX shell. */
export function shellQuote(value: string): string {
  return `'${value.replace(/'/g, `'\\''`)}'`
}

/** The agent's name for the working directory: the repository's own name. */
export function directoryFor(repo: string): string {
  return repo.split('/').pop() ?? repo
}

/**
 * The steps that take a person from an empty repository to an agent that has pushed its first proposal.
 * Returns `null` when the inputs could not be put into a shell line safely (the panel then shows why).
 */
export function connectSteps(input: ConnectInput): ConnectStep[] | null {
  const { repo, subject } = input
  const origin = input.origin.replace(/\/+$/, '')
  if (
    subjectProblem(subject) !== null ||
    !REPO.test(repo) ||
    !/^https?:\/\/[^\s'"\\]+$/.test(origin)
  )
    return null
  const dir = directoryFor(repo)
  const auth = `-H "Authorization: Bearer $${ADMIN_TOKEN_VAR}" -H 'Content-Type: application/json'`
  const role = JSON.stringify({ subject, repo, role: 'writer' })
  const credential = JSON.stringify({
    subject,
    ttl_secs: CREDENTIAL_TTL_SECS,
    scope: { repos: [repo] },
  })
  return [
    {
      id: 'role',
      title: 'Let the agent write to this repository',
      note: 'Grants the writer role on this repository only. Needs an administrator’s token.',
      lines: [
        `curl -sS -X POST ${shellQuote(`${origin}/v1/roles`)} ${auth} -d ${shellQuote(role)}`,
      ],
      admin: true,
    },
    {
      id: 'credential',
      title: 'Mint the agent’s credential',
      note: 'The answer contains the token (agt_…). It is shown once: keep it for step 3. It expires in an hour and works only for this repository.',
      lines: [
        `curl -sS -X POST ${shellQuote(`${origin}/v1/credentials`)} ${auth} -d ${shellQuote(credential)}`,
      ],
      admin: true,
    },
    {
      id: 'work',
      title: 'Run the agent’s first change',
      note: 'Clones the repository, checks who the server thinks you are, snapshots a change and pushes it as a proposal.',
      lines: [
        `export AG_TOKEN=${TOKEN_SLOT}`,
        `ag clone ${shellQuote(`${origin}/${repo}`)} ${shellQuote(dir)}`,
        `cd ${shellQuote(dir)}`,
        'ag remote whoami',
        'echo hi > hello.txt',
        'ag snapshot -m "First change"',
        'ag push -t "First change"',
      ],
      admin: false,
    },
  ]
}

export interface AgentWorkInput {
  origin: string
  /** `owner/name`. */
  repo: string
  /** The agent's secret, `agt_…` (shown once by the server). */
  token: string
  /** What the agent should do, in the task's own words. */
  title: string
  /** Attach the submitted change to this exact task. */
  taskId?: string
}

/**
 * What a person pastes into the agent's shell: the credential, a clone, and the two commands that publish a first
 * change. The values come from the server and from the form, so each goes through `shellQuote`.
 * Returns `null` when the address, repository or token could not be put into a shell line safely.
 */
export function agentWorkLines(input: AgentWorkInput): string[] | null {
  const origin = input.origin.replace(/\/+$/, '')
  if (!REPO.test(input.repo) || !/^https?:\/\/[^\s'"\\]+$/.test(origin)) return null
  if (!/^agt_[A-Za-z0-9_-]+$/.test(input.token)) return null
  const dir = directoryFor(input.repo)
  const title = input.title.replace(/[\r\n]+/g, ' ').trim() || 'Change'
  return [
    `export AG_TOKEN=${shellQuote(input.token)}`,
    `ag clone ${shellQuote(`${origin}/${input.repo}`)} ${shellQuote(dir)}`,
    `cd ${shellQuote(dir)}`,
    'ag remote whoami',
    '# ...the agent makes its changes here...',
    `ag snapshot -m ${shellQuote(title)}`,
    `ag push${input.taskId ? ` --task ${shellQuote(input.taskId)}` : ''} -t ${shellQuote(title)}`,
  ]
}

/** An agent's other tasks and similarly named identities are not progress on this request. */
export function matchesAgentTask(
  expectedTask: string,
  expectedAgent: string,
  task: string,
  identity: string,
): boolean {
  return (
    !!expectedTask &&
    !!expectedAgent &&
    task === expectedTask &&
    samePrincipal(identity, expectedAgent)
  )
}
