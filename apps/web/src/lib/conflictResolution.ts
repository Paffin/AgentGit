import type { Api } from '@/api/endpoints'
import type { RequestOptions } from '@/api/client'
import type { FileChange, TreeEntry } from '@/api/types'
import { putFile, deleteFile } from './edit'
import { decodeUtf8, looksBinary, preserveTextFormat } from './text'

type Session = Pick<RequestOptions, 'token' | 'signal' | 'quietUnauthenticated'>
export type ConflictChoice = '' | 'mine' | 'theirs' | 'delete' | 'edit'
export interface ConflictVersion {
  entry: TreeEntry | null
  /** Null means no editable preview, not an absent file. */
  text: string | null
}
const PREVIEW_BYTES = 512 * 1024

/** Only an explicit missing-path result means deletion. Failed reads stop preparation. */
export async function loadConflictVersion(
  api: Api,
  repo: string,
  rev: string,
  path: string,
  request: Session,
): Promise<ConflictVersion> {
  request.signal?.throwIfAborted()
  const location = await api.tree.resolve(repo, rev, path, request)
  const entry = fileEntry(location, path)
  if (!entry) return { entry: null, text: null }
  if (entry.size > PREVIEW_BYTES) return { entry, text: null }
  request.signal?.throwIfAborted()
  const file = await api.tree.file(repo, rev, path, PREVIEW_BYTES, request)
  return { entry, text: file.truncated || looksBinary(file.bytes) ? null : decodeUtf8(file.bytes) }
}

function fileEntry(
  location: { kind: string; entry: TreeEntry | null },
  path: string,
): TreeEntry | null {
  if (location.kind === 'missing') return null
  const entry = location.entry
  if (!entry || !['regular', 'symlink'].includes(entry.file_type) || !entry.blob_id)
    throw new Error(
      `Cannot inspect ${path} as a file. Resolve this conflict with the command line.`,
    )
  return entry
}

function editedChange(path: string, source: ConflictVersion, text: string): FileChange {
  if (source.entry?.file_type !== 'regular' || source.text === null)
    throw new Error(`This version of ${path} cannot be edited as text.`)
  return putFile(path, preserveTextFormat(source.text, text), source.entry.executable)
}

/** Existing versions are referenced by blob, preserving bytes, type and executable mode. */
export function conflictChange(
  path: string,
  choice: ConflictChoice,
  mine: ConflictVersion,
  theirs: ConflictVersion,
  text: string,
  editSource: 'mine' | 'theirs' = 'mine',
): FileChange {
  if (choice === 'delete') return deleteFile(path)
  if (choice === 'mine' || choice === 'theirs') {
    const entry = (choice === 'mine' ? mine : theirs).entry
    return entry
      ? {
          path,
          op: 'put',
          blob_id: entry.blob_id,
          file_type: entry.file_type,
          executable: entry.executable,
        }
      : deleteFile(path)
  }
  if (choice !== 'edit') throw new Error(`Choose a result for ${path}.`)
  return editedChange(path, editSource === 'mine' ? mine : theirs, text)
}
