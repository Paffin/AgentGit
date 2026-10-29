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
        changes: [preview.change],
        expected_head: head,
        message: `Apply review suggestion to ${preview.change.path}`,
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
