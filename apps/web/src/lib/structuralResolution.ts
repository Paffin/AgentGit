import type { Api } from '@/api/endpoints'
import type { RequestOptions } from '@/api/client'
import type { Conflict, FileChange, TreeEntry } from '@/api/types'
import { pathProblem } from './edit'

export const within = (root: string, path: string) => path === root || path.startsWith(`${root}/`)
export function structuralRoots(conflicts: Conflict[]): string[] {
  if (conflicts.length > 1000)
    throw new Error('Too many conflicts to resolve here. Use the command line.')
  const roots = conflicts
    .filter((c) => c.kind === 'file_directory')
    .map((c) => c.path)
    .sort()
  return roots.filter((path) => !roots.some((other) => other !== path && within(other, path)))
}
export function scopePaths(root: string, scope: Conflict['resolution_scope']): string[] {
  if (scope?.complete !== true || !scope.paths.includes(root))
    throw new Error(
      'The server did not provide a complete folder preview. Resolve this conflict with the command line.',
    )
  if (
    scope.paths.length > 100 ||
    scope.paths.some((path) => !within(root, path) || pathProblem(path))
  )
    throw new Error(
      'The folder preview is invalid or too large. Resolve this conflict with the command line.',
    )
  return [...new Set(scope.paths)].sort()
}
function validEntry(entry: TreeEntry): TreeEntry {
  if (
    !['regular', 'symlink'].includes(entry.file_type) ||
    !entry.blob_id ||
    pathProblem(entry.path)
  )
    throw new Error('Cannot preserve this file type. Resolve this conflict with the command line.')
  return entry
}
export async function loadStructuralSide(
  api: Api,
  repo: string,
  rev: string,
  root: string,
  request: RequestOptions,
  budget = { remaining: 100 },
): Promise<TreeEntry[]> {
  request.signal?.throwIfAborted()
  if (--budget.remaining < 0)
    throw new Error('This folder preview needs too many requests. Use the command line.')
  const location = await api.tree.resolve(repo, rev, root, request)
  if (location.kind === 'missing') return []
  if (location.kind !== 'dir') {
    if (!location.entry || location.entry.path !== root) throw new Error('Invalid file preview.')
    return [validEntry(location.entry)]
  }
  return loadDirectory(api, repo, rev, root, request, budget)
}
async function loadDirectory(
  api: Api,
  repo: string,
  rev: string,
  root: string,
  request: RequestOptions,
  budget: { remaining: number },
): Promise<TreeEntry[]> {
  const files: TreeEntry[] = [],
    dirs = [root]
  let calls = 0
  for (const path of dirs) {
    let page_token = ''
    do {
      request.signal?.throwIfAborted()
      if (++calls > 100 || --budget.remaining < 0)
        throw new Error('This folder is too large to resolve here. Use the command line.')
      const page = await api.tree.list(repo, { rev, path, page_size: 100, page_token }, request)
      appendEntries(page.entries, path, files, dirs)
      page_token = page.nextPageToken
    } while (page_token)
  }
  if (new Set(files.map((file) => file.path)).size !== files.length)
    throw new Error('Invalid duplicate folder entries.')
  return files
}
function appendEntries(
  entries: Awaited<ReturnType<Api['tree']['list']>>['entries'],
  path: string,
  files: TreeEntry[],
  dirs: string[],
) {
  for (const entry of entries) {
    if (!entry.name || entry.name.includes('/') || pathProblem(entry.name))
      throw new Error('Invalid folder preview.')
    const full = `${path}/${entry.name}`
    if (entry.kind === 'dir') dirs.push(full)
    else
      files.push(
        validEntry({
          path: full,
          blob_id: entry.blob_id,
          file_type: entry.kind === 'file' ? 'regular' : entry.kind,
          executable: entry.executable,
          size: entry.size,
        }),
      )
  }
  if (files.length > 100 || dirs.length > 100)
    throw new Error('This folder is too large to resolve here. Use the command line.')
}
export function structuralChanges(
  paths: string[],
  selected: TreeEntry[],
  other: TreeEntry[],
): FileChange[] {
  const keep = new Map(selected.map((entry) => [entry.path, entry]))
  const all = new Set([...paths, ...keep.keys(), ...other.map((entry) => entry.path)])
  return [...all].sort().map((path) => {
    const entry = keep.get(path)
    return entry
      ? {
          path,
          op: 'put',
          blob_id: entry.blob_id,
          file_type: entry.file_type,
          executable: entry.executable,
        }
      : { path, op: 'delete' }
  })
}
