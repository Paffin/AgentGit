// How a conflict is shown: the server's own `message` when it has one (it carries the details), otherwise a plain
// reading of the stable `kind` code. The two directory-rename codes (ag-merge/3, ADR-0013) say what happened to a
// whole directory, so they get a sentence of their own; an unknown future code falls back to its spelling.

const WORDS: Record<string, string> = {
  directory_rename_ambiguous:
    'directory rename: the files of the old directory went to more than one place',
  directory_rename_collision:
    'directory rename: a moved file lands on a path that is already taken',
}

export interface ConflictLike {
  kind: string
  message?: string
}

export function conflictLabel(c: ConflictLike): string {
  return c.message || WORDS[c.kind] || c.kind.replaceAll('_', ' ')
}

export function isDirectoryRenameConflict(c: ConflictLike): boolean {
  return c.kind.startsWith('directory_rename_')
}
