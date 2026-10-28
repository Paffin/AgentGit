// Roles, as a hint for the UI. The server is the authority: it answers 403 for anything not allowed.
// The UI uses roles only to hide controls that could never work and to say *why* they are unavailable.
import type { Role } from '@/api/types'
import { ROLES } from '@/api/types'
import { repoPatternMatches } from './glob'

export interface RoleGrant {
  pattern: string
  role: Role
}

/** "acme/*:writer" -> { pattern: "acme/*", role: "writer" }. Unknown roles are dropped. */
export function parseRoleGrants(roles: readonly string[]): RoleGrant[] {
  const out: RoleGrant[] = []
  for (const raw of roles) {
    const at = raw.lastIndexOf(':')
    if (at <= 0) continue
    const role = raw.slice(at + 1) as Role
    if (!ROLES.includes(role)) continue
    out.push({ pattern: raw.slice(0, at), role })
  }
  return out
}

export function rolesForRepo(grants: readonly RoleGrant[], repo: string): Set<Role> {
  const set = new Set<Role>()
  for (const g of grants) if (repoPatternMatches(g.pattern, repo)) set.add(g.role)
  return set
}

/** Does holding `have` satisfy a requirement for `need`? `admin` grants everything; any role can read. */
export function satisfies(have: ReadonlySet<Role>, need: Role): boolean {
  if (have.size === 0) return false
  if (have.has('admin')) return true
  if (need === 'reader') return true
  if (need === 'writer' && have.has('delegator')) return true
  return have.has(need)
}

/** What the UI asks of the caller for each action (mirrors the table in docs/protocol/agwp-v1.md). */
export const ACTION_ROLE = {
  review: 'reviewer',
  integrate: 'integrator',
  runQueue: 'integrator',
  enqueue: 'integrator',
  closeProposal: 'integrator',
  requestValidation: 'writer',
  createTask: 'writer',
  createIssue: 'writer',
  editFiles: 'writer',
  connectAgent: 'admin',
  manageLeases: 'writer',
  registerAgent: 'writer',
  createRepo: 'admin',
  admin: 'admin',
} as const satisfies Record<string, Role>

export type Action = keyof typeof ACTION_ROLE

/** Roles that may do an action (the table lists writers for some and integrators for others). */
const ALSO_ALLOWED: Partial<Record<Action, readonly Role[]>> = {
  enqueue: ['writer'],
  closeProposal: ['writer'],
  requestValidation: ['integrator'],
}

export function mayDo(have: ReadonlySet<Role>, action: Action): boolean {
  if (satisfies(have, ACTION_ROLE[action])) return true
  return (ALSO_ALLOWED[action] ?? []).some((r) => have.has(r))
}

export function requirementText(action: Action): string {
  const roles = [ACTION_ROLE[action], ...(ALSO_ALLOWED[action] ?? [])]
  return `Requires the ${roles.join(' or ')} role`
}
