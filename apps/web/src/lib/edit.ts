// Editing files from the browser, over the server's virtual workspaces (a private working copy of a branch kept on
// the server): open one, apply the edit as one version, then open a change request from it. Nothing here is
// special to the web: it is the same calls any client without a file system makes (docs/protocol/agwp-v1.md,
// "Objects and publication").
import type { Api } from '@/api/endpoints'
import type { FileChange, Proposal } from '@/api/types'

/** UTF-8 text as the base64 the protocol carries file contents in. */
export function toBase64(text: string): string {
  const bytes = new TextEncoder().encode(text)
  let bin = ''
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk)
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk))
  return btoa(bin)
}

export const putFile = (path: string, text: string, executable = false): FileChange => ({
  path,
  op: 'put',
  content: toBase64(text),
  executable,
})
export const deleteFile = (path: string): FileChange => ({ path, op: 'delete' })

/** Why `path` cannot be a file in a repository, or `null` when it can. */
export function pathProblem(path: string): string | null {
  const p = path.trim()
  if (!p) return 'Write the file’s path, for example docs/intro.md.'
  if (p.startsWith('/')) return 'Leave out the leading slash.'
  if (p.endsWith('/')) return 'A file’s path does not end with a slash.'
  if (p.length > 1024) return 'This path is too long.'
  if (/[\0\\]/.test(p)) return 'Use forward slashes and no special characters.'
  if (p.split('/').some((s) => s === '' || s === '.' || s === '..'))
    return 'Leave out empty parts and “.” or “..”.'
  if (p.split('/').includes('.ag') || p.split('/').includes('.git'))
    return 'Names like .ag and .git are reserved.'
  return null
}

/** A workspace name nobody else is using: the workspace is private and short-lived. */
export function workspaceName(random: (n: number) => Uint8Array = randomBytes): string {
  return `web-${[...random(5)].map((b) => b.toString(16).padStart(2, '0')).join('')}`
}
function randomBytes(n: number): Uint8Array {
  const a = new Uint8Array(n)
  crypto.getRandomValues(a)
  return a
}

export interface ProposeInput {
  repo: string
  /** The branch the change is for. */
  stream: string
  /** Start from this revision instead of the branch's head (the revision that was on screen). */
  fromRev?: string
  changes: FileChange[]
  title: string
  description?: string
}

/**
 * Opens a draft, applies the changes as one version, opens a change request for it, and removes the draft (the
 * change request keeps the version). Returns the new change request. A failure after the draft exists removes it.
 */
export async function proposeChange(api: Api, input: ProposeInput): Promise<Proposal> {
  const name = workspaceName()
  const ws = await api.workspaces.create(input.repo, {
    name,
    stream: input.stream,
    from_rev: input.fromRev,
  })
  try {
    const done = await api.workspaces.modify(input.repo, name, {
      changes: input.changes,
      message: input.title,
      expected_head: ws.head,
    })
    return await api.proposals.create(input.repo, {
      stream: input.stream,
      head: done.head,
      title: input.title,
      description: input.description,
    })
  } finally {
    try {
      await api.workspaces.remove(input.repo, name)
    } catch {
      // A leftover draft is harmless and visible to its owner; the change request is what matters.
    }
  }
}

export type ResolvableKind = 'content' | 'both_added' | 'delete_modify'
const RESOLVABLE = new Set<string>(['content', 'both_added', 'delete_modify'])
/** Conflicts a person can settle by choosing or editing text; the rest (binary, type changes, renames) need the CLI. */
export const canResolveHere = (kind: string): boolean => RESOLVABLE.has(kind)
