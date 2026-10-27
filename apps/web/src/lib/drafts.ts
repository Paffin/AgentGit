/** Drafts belong to one signed-in account and one browser tab; credentials are never copied here. */
export interface DraftScope {
  identity: string
  repo: string
  stream: string
  path: string
  isNew: boolean
}
export interface FileDraft {
  version: 1
  text: string
  original: string
  path: string
  base: string
  executable: boolean
  fileType: 'regular' | 'symlink'
  title: string
  description: string
  savedAt: number
}
export type DraftStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>

export function draftKey(scope: DraftScope): string {
  if (!scope.identity) throw new Error('Sign in before saving a draft.')
  return `agentgit.draft.v1:${JSON.stringify([scope.identity, scope.repo, scope.stream, scope.path, scope.isNew])}`
}

function validDraft(value: unknown): value is FileDraft {
  if (!value) return false
  if (typeof value !== 'object') return false
  const d = value as Record<string, unknown>
  const strings = ['text', 'original', 'path', 'base', 'title', 'description']
  return d.version === 1 && strings.every((key) => typeof d[key] === 'string') && validMode(d)
}
function validMode(d: Record<string, unknown>) {
  return (
    typeof d.executable === 'boolean' &&
    (d.fileType === 'regular' || d.fileType === 'symlink') &&
    typeof d.savedAt === 'number' &&
    Number.isFinite(d.savedAt) &&
    d.savedAt >= 0
  )
}

export function readDraft(storage: DraftStorage, scope: DraftScope): FileDraft | null {
  try {
    const raw = storage.getItem(draftKey(scope))
    if (raw === null) return null
    const value: unknown = JSON.parse(raw)
    if (!validDraft(value)) throw new Error('Invalid draft format')
    return value
  } catch {
    throw new Error('The saved draft could not be read. It has been left untouched.')
  }
}

export function saveDraft(
  storage: DraftStorage,
  scope: DraftScope,
  draft: Omit<FileDraft, 'version'>,
): void {
  const { text, original, path, base, executable, fileType, title, description, savedAt } = draft
  try {
    storage.setItem(
      draftKey(scope),
      JSON.stringify({
        version: 1,
        text,
        original,
        path,
        base,
        executable,
        fileType,
        title,
        description,
        savedAt,
      }),
    )
  } catch {
    throw new Error(
      'This tab could not save your draft. Download a copy before leaving or reloading.',
    )
  }
}

export function removeDraft(storage: DraftStorage, scope: DraftScope): void {
  storage.removeItem(draftKey(scope))
}
