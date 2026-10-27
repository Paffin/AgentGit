import type { Api } from '@/api/endpoints'
import type { FileChange } from '@/api/types'
import { bytesToBase64, pathProblem } from './edit'

export const UPLOAD_LIMIT = 4 * 1024 * 1024
export const UPLOAD_FILES = 100
export interface UploadFile {
  file: File
  path: string
}
export interface CheckedUpload extends UploadFile {
  replaces: boolean
  executable: boolean
}

export function uploadFiles(files: readonly File[], folder: string): UploadFile[] {
  if (files.length > UPLOAD_FILES)
    throw new Error(`Choose at most ${UPLOAD_FILES} files per change request.`)
  if (files.reduce((size, file) => size + file.size, 0) > UPLOAD_LIMIT)
    throw new Error('Choose at most 4 MiB of files per change request.')
  const prefix = folder.trim().replace(/\/+$/, '')
  const seen = new Set<string>()
  return files.map((file) => {
    const path = [prefix, file.webkitRelativePath || file.name].filter(Boolean).join('/')
    const problem = pathProblem(path)
    if (problem) throw new Error(`${path}: ${problem}`)
    if (seen.has(path)) throw new Error(`Two files have the same destination: ${path}`)
    seen.add(path)
    return { file, path }
  })
}

export async function checkUploads(
  api: Api,
  repo: string,
  rev: string,
  files: UploadFile[],
): Promise<CheckedUpload[]> {
  const checked: CheckedUpload[] = []
  // Bound requests to avoid flooding the server for a folder upload.
  for (let offset = 0; offset < files.length; offset += 5) {
    checked.push(
      ...(await Promise.all(
        files.slice(offset, offset + 5).map(async (file) => {
          if (!rev) return { ...file, replaces: false, executable: false }
          const existing = await api.tree.resolve(repo, rev, file.path)
          if (existing.kind === 'dir')
            throw new Error(`${file.path} is already a folder. Choose another destination.`)
          return {
            ...file,
            replaces: existing.kind !== 'missing',
            executable: existing.entry?.executable === true,
          }
        }),
      )),
    )
  }
  return checked
}
export async function uploadChanges(files: CheckedUpload[]): Promise<FileChange[]> {
  const changes: FileChange[] = []
  for (const item of files) {
    const bytes = new Uint8Array(await item.file.arrayBuffer())
    changes.push({
      path: item.path,
      op: 'put',
      content: bytesToBase64(bytes),
      file_type: 'regular',
      executable: item.executable,
    })
  }
  return changes
}
