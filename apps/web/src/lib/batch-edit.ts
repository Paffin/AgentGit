import type { FileChange } from '@/api/types'
import { deleteFile, pathProblem, putFile } from './edit'
import { preserveTextFormat } from './text'

export interface DraftFile {
  id: string
  originalPath: string | null
  path: string
  original: string
  text: string
  executable: boolean
  fileType: 'regular' | 'symlink'
  removed: boolean
}
export interface BatchDraft {
  version: 1
  base: string
  files: DraftFile[]
  title: string
  description: string
}
export const BATCH_LIMIT = 100
export const BATCH_BYTES = 4 * 1024 * 1024
export function fileStatus(file: DraftFile): string {
  if (file.removed) return file.originalPath ? 'Deleted' : 'Discarded'
  if (!file.originalPath) return 'New'
  if (file.path !== file.originalPath) return 'Renamed'
  return file.text !== file.original ? 'Modified' : 'Unchanged'
}
export function batchChanges(files: DraftFile[]): FileChange[] {
  if (files.length > BATCH_LIMIT)
    throw new Error(`A change can contain at most ${BATCH_LIMIT} files.`)
  const paths = new Set<string>()
  const deleted = new Set<string>()
  const puts: FileChange[] = []
  let bytes = 0
  for (const file of files) {
    if (file.removed) {
      if (file.originalPath) deleted.add(file.originalPath)
      continue
    }
    const problem = pathProblem(file.path)
    if (problem) throw new Error(`File path ${file.path}: ${problem}`)
    if (paths.has(file.path)) throw new Error('Two files cannot use the same path.')
    paths.add(file.path)
    if (fileStatus(file) === 'Unchanged') continue
    const text = preserveTextFormat(file.original, file.text)
    bytes += new TextEncoder().encode(text).length
    if (bytes > BATCH_BYTES)
      throw new Error('The changed contents exceed 4 MiB. Split this change into smaller parts.')
    if (file.originalPath && file.path !== file.originalPath) deleted.add(file.originalPath)
    puts.push(
      putFile(
        file.path,
        text,
        file.executable,
        file.fileType === 'symlink' ? 'symlink' : undefined,
      ),
    )
  }
  return [...deleted].map(deleteFile).concat(puts)
}
function validFile(value: unknown): value is DraftFile {
  if (!value || typeof value !== 'object') return false
  const file = value as Record<string, unknown>
  const strings = ['id', 'path', 'original', 'text']
  return (
    strings.every((key) => typeof file[key] === 'string') &&
    (file.originalPath === null || typeof file.originalPath === 'string') &&
    typeof file.executable === 'boolean' &&
    typeof file.removed === 'boolean' &&
    (file.fileType === 'regular' || file.fileType === 'symlink')
  )
}
export function restoreBatch(raw: string | null): BatchDraft | null {
  if (!raw) return null
  if (raw.length > BATCH_BYTES * 3) throw new Error('The saved draft is too large to read safely.')
  let value: unknown
  try {
    value = JSON.parse(raw)
  } catch {
    throw new Error('The saved draft is not valid JSON.')
  }
  if (!value || typeof value !== 'object') throw new Error('The saved draft is invalid.')
  const draft = value as Record<string, unknown>
  if (
    draft.version !== 1 ||
    typeof draft.base !== 'string' ||
    typeof draft.title !== 'string' ||
    typeof draft.description !== 'string'
  )
    throw new Error('The saved draft is invalid.')
  if (
    !Array.isArray(draft.files) ||
    draft.files.length > BATCH_LIMIT ||
    !draft.files.every(validFile)
  )
    throw new Error('The saved draft has invalid files.')
  if (new Set(draft.files.map((f) => f.id)).size !== draft.files.length)
    throw new Error('The saved draft has duplicate file identifiers.')
  return {
    version: 1,
    base: draft.base,
    title: draft.title,
    description: draft.description,
    files: draft.files,
  }
}
