import type { FileChange, TreeEntry } from '@/api/types'
import { deleteFile, pathProblem } from './edit'

/** A move reuses the exact blob and mode; binary data and symlinks are never decoded. */
export function moveFile(entry: TreeEntry, destination: string): FileChange[] {
  const target = destination.trim()
  const problem = pathProblem(target)
  if (problem) throw new Error(problem)
  if (target === entry.path) throw new Error('Choose a different file path.')
  return [
    deleteFile(entry.path),
    {
      path: target,
      op: 'put',
      blob_id: entry.blob_id,
      executable: entry.executable,
      file_type: entry.file_type,
    },
  ]
}
