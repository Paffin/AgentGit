// A tolerant parser for the unified diffs the server returns in `FileDiff.patch`. It produces plain data;
// the viewer renders every string as text, so nothing in a diff can ever become markup.

export type DiffLineKind = 'add' | 'del' | 'ctx' | 'meta'

export interface DiffLine {
  kind: DiffLineKind
  /** The line without its leading `+`, `-` or space. */
  text: string
  oldNo: number | null
  newNo: number | null
}

export interface DiffHunk {
  header: string
  oldStart: number
  oldCount: number
  newStart: number
  newCount: number
  /** Text after the second `@@` (git puts the enclosing function there). */
  section: string
  lines: DiffLine[]
}

export interface ParsedPatch {
  /** Lines before the first hunk (`--- a/x`, `+++ b/x`, …). */
  preamble: string[]
  hunks: DiffHunk[]
  additions: number
  deletions: number
}

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/

export function parseUnifiedDiff(patch: string): ParsedPatch {
  const result: ParsedPatch = { preamble: [], hunks: [], additions: 0, deletions: 0 }
  if (!patch) return result
  const raw = patch.split('\n')
  if (raw.length > 0 && raw[raw.length - 1] === '') raw.pop()
  let hunk: DiffHunk | null = null
  let oldNo = 0
  let newNo = 0
  for (const line of raw) {
    const m = HUNK.exec(line)
    if (m) {
      hunk = {
        header: line,
        oldStart: Number(m[1]),
        oldCount: m[2] === undefined ? 1 : Number(m[2]),
        newStart: Number(m[3]),
        newCount: m[4] === undefined ? 1 : Number(m[4]),
        section: m[5] ?? '',
        lines: [],
      }
      oldNo = hunk.oldStart
      newNo = hunk.newStart
      result.hunks.push(hunk)
      continue
    }
    if (!hunk) {
      result.preamble.push(line)
      continue
    }
    const sigil = line.charAt(0)
    const text = line.slice(1)
    if (sigil === '+') {
      hunk.lines.push({ kind: 'add', text, oldNo: null, newNo: newNo++ })
      result.additions++
    } else if (sigil === '-') {
      hunk.lines.push({ kind: 'del', text, oldNo: oldNo++, newNo: null })
      result.deletions++
    } else if (sigil === '\\') {
      hunk.lines.push({
        kind: 'meta',
        text: line.replace(/^\\ ?/, ''),
        oldNo: null,
        newNo: null,
      })
    } else if (sigil === ' ' || line === '') {
      hunk.lines.push({ kind: 'ctx', text, oldNo: oldNo++, newNo: newNo++ })
    } else {
      hunk.lines.push({ kind: 'meta', text: line, oldNo: null, newNo: null })
    }
  }
  return result
}

/** Total number of rendered lines across hunks (for collapsing huge diffs). */
export function lineCount(p: ParsedPatch): number {
  return p.hunks.reduce((n, h) => n + h.lines.length + 1, 0)
}
