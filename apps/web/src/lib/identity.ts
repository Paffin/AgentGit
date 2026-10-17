import type { IdentityKind } from '@/api/types'

export interface ParsedIdentity {
  raw: string
  kind: IdentityKind
  /** Name without kind or run suffix: `refactor-bot`. */
  name: string
  /** `agent:refactor-bot` — the principal rules compare (self-approval, lease ownership). */
  principal: string
  /** `run_…` when this is an agent run identity. */
  run: string
}

const KINDS = new Set(['user', 'agent', 'service', 'system'])

/** `kind:name` or `kind:name#run_id`. Unknown shapes are kept verbatim as kind `unknown`. */
export function parseIdentity(raw: string): ParsedIdentity {
  const hash = raw.indexOf('#')
  const principal = hash === -1 ? raw : raw.slice(0, hash)
  const run = hash === -1 ? '' : raw.slice(hash + 1)
  const colon = principal.indexOf(':')
  const kindWord = colon === -1 ? '' : principal.slice(0, colon)
  if (!KINDS.has(kindWord)) {
    return { raw, kind: 'unknown', name: principal, principal, run }
  }
  return { raw, kind: kindWord as IdentityKind, name: principal.slice(colon + 1), principal, run }
}

export function isHuman(identity: string): boolean {
  return parseIdentity(identity).kind === 'user'
}

export function samePrincipal(a: string, b: string): boolean {
  return parseIdentity(a).principal === parseIdentity(b).principal
}

export const KIND_LABEL: Record<IdentityKind, string> = {
  user: 'Person',
  agent: 'Agent',
  service: 'Service',
  system: 'System',
  unknown: 'Identity',
}
