// A small line diff (longest common subsequence) for comparing two versions of a text document, such as
// two versions of a stream policy. Not used for repository diffs: the server computes those.

export interface LineOp {
  kind: 'eq' | 'add' | 'del'
  text: string
  aNo: number | null
  bNo: number | null
}

const MAX_CELLS = 4_000_000

export function diffLines(a: readonly string[], b: readonly string[]): LineOp[] {
  let start = 0
  while (start < a.length && start < b.length && a[start] === b[start]) start++
  let endA = a.length
  let endB = b.length
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--
    endB--
  }
  const out: LineOp[] = []
  for (let i = 0; i < start; i++) out.push({ kind: 'eq', text: a[i] ?? '', aNo: i + 1, bNo: i + 1 })

  const n = endA - start
  const m = endB - start
  if (n * m > MAX_CELLS) {
    for (let i = start; i < endA; i++)
      out.push({ kind: 'del', text: a[i] ?? '', aNo: i + 1, bNo: null })
    for (let j = start; j < endB; j++)
      out.push({ kind: 'add', text: b[j] ?? '', aNo: null, bNo: j + 1 })
  } else {
    const w = m + 1
    const table = new Int32Array((n + 1) * w)
    for (let i = n - 1; i >= 0; i--) {
      for (let j = m - 1; j >= 0; j--) {
        table[i * w + j] =
          a[start + i] === b[start + j]
            ? (table[(i + 1) * w + j + 1] ?? 0) + 1
            : Math.max(table[(i + 1) * w + j] ?? 0, table[i * w + j + 1] ?? 0)
      }
    }
    let i = 0
    let j = 0
    while (i < n && j < m) {
      if (a[start + i] === b[start + j]) {
        out.push({
          kind: 'eq',
          text: a[start + i] ?? '',
          aNo: start + i + 1,
          bNo: start + j + 1,
        })
        i++
        j++
      } else if ((table[(i + 1) * w + j] ?? 0) >= (table[i * w + j + 1] ?? 0)) {
        out.push({ kind: 'del', text: a[start + i] ?? '', aNo: start + i + 1, bNo: null })
        i++
      } else {
        out.push({ kind: 'add', text: b[start + j] ?? '', aNo: null, bNo: start + j + 1 })
        j++
      }
    }
    for (; i < n; i++)
      out.push({ kind: 'del', text: a[start + i] ?? '', aNo: start + i + 1, bNo: null })
    for (; j < m; j++)
      out.push({ kind: 'add', text: b[start + j] ?? '', aNo: null, bNo: start + j + 1 })
  }
  for (let k = 0; endA + k < a.length; k++) {
    out.push({ kind: 'eq', text: a[endA + k] ?? '', aNo: endA + k + 1, bNo: endB + k + 1 })
  }
  return out
}

export function diffText(a: string, b: string): LineOp[] {
  return diffLines(a.split('\n'), b.split('\n'))
}

export function diffStats(ops: readonly LineOp[]): { additions: number; deletions: number } {
  let additions = 0
  let deletions = 0
  for (const op of ops) {
    if (op.kind === 'add') additions++
    else if (op.kind === 'del') deletions++
  }
  return { additions, deletions }
}
