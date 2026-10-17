import type { BlameLine } from '@/api/types'

/** Consecutive lines that came from the same changeset (and the same file name), shown as one block. */
export interface BlameRun {
  changeset: string
  author: string
  summary: string
  timestampMs: number
  /** The file's name in that changeset; differs from the current one after a rename. */
  path: string
  /** The walk stopped before finding the real origin: the line is "older than shown". */
  boundary: boolean
  lines: BlameLine[]
}

export function groupBlame(lines: readonly BlameLine[]): BlameRun[] {
  const runs: BlameRun[] = []
  for (const line of lines) {
    const last = runs[runs.length - 1]
    if (
      last &&
      last.changeset === line.changeset &&
      last.boundary === line.boundary &&
      last.path === line.path
    ) {
      last.lines.push(line)
      continue
    }
    runs.push({
      changeset: line.changeset,
      author: line.author,
      summary: line.summary,
      timestampMs: line.timestamp_ms,
      path: line.path,
      boundary: line.boundary,
      lines: [line],
    })
  }
  return runs
}
