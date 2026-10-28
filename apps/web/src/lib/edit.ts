// Editing files from the browser, over the server's virtual workspaces (an isolated working copy of a branch kept on
// the server): open one, apply the edit as one version, then open a change request from it. Nothing here is
// special to the web: it is the same calls any client without a file system makes (docs/protocol/agwp-v1.md,
// "Objects and publication").
import type { Api } from '@/api/endpoints'
import type { RequestOptions } from '@/api/client'
import type { FileChange, Proposal } from '@/api/types'

/** UTF-8 text as the base64 the protocol carries file contents in. */
export function toBase64(text: string): string {
  return bytesToBase64(new TextEncoder().encode(text))
}

export function bytesToBase64(bytes: Uint8Array): string {
  let bin = ''
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk)
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk))
  return btoa(bin)
}

export const putFile = (
  path: string,
  text: string,
  executable = false,
  fileType?: 'symlink',
): FileChange => ({
  path,
  op: 'put',
  content: toBase64(text),
  executable,
  ...(fileType ? { file_type: fileType } : {}),
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

/** A workspace name nobody else is using: the workspace is isolated and short-lived. */
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
  /** The base shown to the editor; an empty string means the branch had no commits. */
  expectedBase?: string
  changes: FileChange[]
  title: string
  description?: string
  request?: Pick<RequestOptions, 'token' | 'quietUnauthenticated' | 'signal'>
}

/**
 * Opens a draft, applies the changes as one version, opens a change request for it, and removes the draft (the
 * change request keeps the version). Returns the new change request. A failure after the draft exists removes it.
 */
export async function proposeChange(api: Api, input: ProposeInput): Promise<Proposal> {
  const request = input.request ?? {}
  request.signal?.throwIfAborted()
  const name = workspaceName()
  const ws = await api.workspaces.create(
    input.repo,
    {
      name,
      stream: input.stream,
      from_rev: input.fromRev,
    },
    request,
  )
  try {
    request.signal?.throwIfAborted()
    if (input.expectedBase !== undefined && ws.base !== input.expectedBase) {
      throw new Error(
        'The branch changed before this edit could start. Reload it and try again; your text is still here.',
      )
    }
    const done = await api.workspaces.modify(
      input.repo,
      name,
      {
        changes: input.changes,
        message: input.title,
        expected_head: ws.head,
      },
      request,
    )
    request.signal?.throwIfAborted()
    return await api.proposals.create(
      input.repo,
      {
        stream: input.stream,
        head: done.head,
        title: input.title,
        description: input.description,
      },
      request,
    )
  } finally {
    try {
      await api.workspaces.remove(input.repo, name, {
        token: request.token,
        quietUnauthenticated: request.quietUnauthenticated,
        timeoutMs: 5000,
      })
    } catch {
      // A leftover draft is harmless and visible to its owner; the change request is what matters.
    }
  }
}

export type ResolvableKind = 'content' | 'both_added' | 'delete_modify'
const RESOLVABLE = new Set<string>(['content', 'both_added', 'delete_modify'])
/** Conflicts a person can settle by choosing or editing text; the rest (binary, type changes, renames) need the CLI. */
export const canResolveHere = (kind: string): boolean => RESOLVABLE.has(kind)
