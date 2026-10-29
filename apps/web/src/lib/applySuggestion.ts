import type { Api } from '@/api/endpoints'
import type { RequestOptions } from '@/api/client'
import type { FileChange, ReviewLineAnchor } from '@/api/types'
import { putFile, workspaceName } from './edit'
import { replaceSuggestedLine } from './reviewSuggestion'

type Session = Pick<RequestOptions, 'token' | 'signal' | 'quietUnauthenticated'>
export interface SuggestionPreview {
  before: string
  after: string
  change: FileChange
}

export async function previewSuggestion(
  api: Api,
  repo: string,
  anchor: ReviewLineAnchor,
  replacement: string,
  request: Session,
): Promise<SuggestionPreview> {
  if (anchor.side !== 'new')
    throw new Error('Select a line in the proposed version to suggest a replacement.')
  const [location, file] = await Promise.all([
    api.tree.resolve(repo, anchor.head, anchor.path, request),
    api.tree.file(repo, anchor.head, anchor.path, 1024 * 1024, request),
  ])
  if (location.entry?.file_type !== 'regular' || file.truncated)
    throw new Error(
      'Suggestions apply to regular text files up to 1 MiB. Use the file editor for this file.',
    )
  const before = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(file.bytes)
  const after = replaceSuggestedLine(before, anchor.line, anchor.text, replacement)
  if (after === before) throw new Error('This suggestion does not change the file.')
  return { before, after, change: putFile(anchor.path, after, location.entry.executable) }
}

export interface SuggestionInput {
  anchor: ReviewLineAnchor
  replacement: string
}

/** Prepare one change per file, applying original line numbers from bottom to top. */
export async function previewSuggestions(
  api: Api,
  repo: string,
  head: string,
  suggestions: SuggestionInput[],
  request: Session,
): Promise<SuggestionPreview[]> {
  const files = groupSuggestions(head, suggestions)
  const result: SuggestionPreview[] = []
  let bytes = 0
  for (const path of [...files.keys()].sort()) {
    request.signal?.throwIfAborted()
    const preview = await previewFileSuggestions(api, repo, files.get(path)!, request)
    bytes += new TextEncoder().encode(preview.after).length
    if (bytes > 4 * 1024 * 1024)
      throw new Error('The selected changes exceed 4 MiB. Select fewer suggestions.')
    result.push(preview)
  }
  return result
}

function groupSuggestions(
  head: string,
  suggestions: SuggestionInput[],
): Map<string, SuggestionInput[]> {
  if (!suggestions.length || suggestions.length > 100)
    throw new Error('Select between 1 and 100 suggestions.')
  const seen = new Set<string>()
  const files = new Map<string, SuggestionInput[]>()
  for (const suggestion of suggestions) {
    const anchor = suggestion.anchor
    if (anchor.head !== head || anchor.side !== 'new')
      throw new Error('Every suggestion must belong to the current proposed revision.')
    const key = JSON.stringify([anchor.path, anchor.line])
    if (seen.has(key)) throw new Error('Choose only one suggestion for the same line.')
    seen.add(key)
    const group = files.get(anchor.path) || []
    group.push(suggestion)
    files.set(anchor.path, group)
  }
  return files
}

/** Each anchor still addresses the immutable original file's line numbers. */
async function previewFileSuggestions(
  api: Api,
  repo: string,
  suggestions: SuggestionInput[],
  request: Session,
): Promise<SuggestionPreview> {
  const group = [...suggestions].sort((a, b) => b.anchor.line - a.anchor.line)
  const first = group[0]!
  const preview = await previewSuggestion(api, repo, first.anchor, first.replacement, request)
  for (const suggestion of group.slice(1))
    preview.after = replaceSuggestedLine(
      preview.after,
      suggestion.anchor.line,
      suggestion.anchor.text,
      suggestion.replacement,
    )
  preview.change = putFile(first.anchor.path, preview.after, preview.change.executable)
  return preview
}

/** A private workspace prepares content; only a head-checked update publishes it to the proposal. */
export async function applySuggestion(
  api: Api,
  repo: string,
  proposal: string,
  stream: string,
  head: string,
  preview: SuggestionPreview,
  request: Session,
): Promise<void> {
  return applySuggestions(api, repo, proposal, stream, head, [preview], request)
}

/** Publish the complete preview in one changeset with one inspected-head update. */
export async function applySuggestions(
  api: Api,
  repo: string,
  proposal: string,
  stream: string,
  head: string,
  previews: SuggestionPreview[],
  request: Session,
): Promise<void> {
  if (!previews.length || previews.length > 100) throw new Error('Select between 1 and 100 files.')
  request.signal?.throwIfAborted()
  const name = workspaceName()
  const workspace = await api.workspaces.create(repo, { name, stream, from_rev: head }, request)
  try {
    request.signal?.throwIfAborted()
    if (workspace.head !== head)
      throw new Error('The suggestion workspace did not open at the inspected revision.')
    const changed = await api.workspaces.modify(
      repo,
      name,
      {
        changes: previews.map((preview) => preview.change),
        expected_head: head,
        message:
          previews.length === 1
            ? `Apply review suggestion to ${previews[0]!.change.path}`
            : 'Apply review suggestions',
      },
      request,
    )
    request.signal?.throwIfAborted()
    await api.proposals.update(repo, proposal, changed.head, head, request)
  } finally {
    try {
      await api.workspaces.remove(repo, name, {
        token: request.token,
        quietUnauthenticated: true,
        timeoutMs: 5000,
      })
    } catch {
      // The retained workspace is visible to its owner and does not move a branch or proposal.
    }
  }
}
