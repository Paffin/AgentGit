// What the global setup learned, shared with the test workers through a small JSON file.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import type { Demo } from './seed.ts'
import type { CiRules } from './world.ts'
import { Cast, readRules } from './world.ts'

export const WEB_ROOT = resolve(import.meta.dirname, '../..')
export const RUNTIME_DIR = join(WEB_ROOT, 'e2e', '.runtime')
const STATE_FILE = join(RUNTIME_DIR, 'state.json')
const RULES_FILE = join(RUNTIME_DIR, 'ci-rules.json')

export interface State {
  baseURL: string
  tokens: Record<string, string>
  demo: Demo
  hostile: { proposal: string }
  /** The fake identity provider the server trusts and the UI offers a sign-in button for. */
  idp: { issuer: string; clientId: string }
}

export function writeState(state: State): void {
  mkdirSync(RUNTIME_DIR, { recursive: true })
  writeFileSync(STATE_FILE, JSON.stringify(state, null, 2))
}

export function readState(): State {
  if (!existsSync(STATE_FILE)) {
    throw new Error(`${STATE_FILE} is missing: the global setup did not run.`)
  }
  return JSON.parse(readFileSync(STATE_FILE, 'utf8')) as State
}

export function castOf(state: State = readState()): Cast {
  return new Cast(state.baseURL, state.tokens)
}

// ---- the CI stand-in's rules (read by the worker in the setup process before every job) -----------

export function setCiRules(rules: CiRules): void {
  mkdirSync(RUNTIME_DIR, { recursive: true })
  writeFileSync(RULES_FILE, JSON.stringify({ ...readRules(RUNTIME_DIR), ...rules }))
}

/** From now on the CI stand-in reports a failing check for this proposal. */
export function failChecksFor(proposal: string): void {
  const rules = readRules(RUNTIME_DIR)
  setCiRules({ failProposals: [...(rules.failProposals ?? []), proposal] })
}
