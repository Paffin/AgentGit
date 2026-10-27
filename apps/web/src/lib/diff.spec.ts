// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { lineCount, parseUnifiedDiff } from './unidiff'
import { diffLines, diffStats, diffText } from './linediff'

describe('parseUnifiedDiff', () => {
  const patch = [
    '--- a/src/a.ts',
    '+++ b/src/a.ts',
    '@@ -1,4 +1,5 @@ function a()',
    ' keep',
    '-old line',
    '+new line',
    '+another',
    ' tail',
    '\\ No newline at end of file',
    '@@ -20 +21,2 @@',
    ' ctx',
    '+added',
    '',
  ].join('\n')

  it('splits hunks, numbers lines and counts changes', () => {
    const p = parseUnifiedDiff(patch)
    expect(p.preamble).toEqual(['--- a/src/a.ts', '+++ b/src/a.ts'])
    expect(p.hunks).toHaveLength(2)
    const h = p.hunks[0]!
    expect(h).toMatchObject({
      oldStart: 1,
      oldCount: 4,
      newStart: 1,
      newCount: 5,
      section: 'function a()',
    })
    expect(h.lines.map((l) => [l.kind, l.oldNo, l.newNo])).toEqual([
      ['ctx', 1, 1],
      ['del', 2, null],
      ['add', null, 2],
      ['add', null, 3],
      ['ctx', 3, 4],
      ['meta', null, null],
    ])
    expect(p.additions).toBe(3)
    expect(p.deletions).toBe(1)
    expect(p.hunks[1]).toMatchObject({ oldStart: 20, oldCount: 1, newStart: 21, newCount: 2 })
    expect(lineCount(p)).toBe(6 + 1 + 2 + 1)
  })

  it('treats a leading "--" in content as content, not as a file header', () => {
    const p = parseUnifiedDiff('--- a/x\n+++ b/x\n@@ -1 +0,0 @@\n--- removed sql comment\n')
    expect(p.hunks[0]!.lines[0]).toMatchObject({ kind: 'del', text: '-- removed sql comment' })
  })

  it('keeps hostile text as plain strings', () => {
    const p = parseUnifiedDiff('@@ -0,0 +1 @@\n+<img src=x onerror=alert(1)>\n')
    expect(p.hunks[0]!.lines[0]!.text).toBe('<img src=x onerror=alert(1)>')
  })

  it('copes with empty and malformed input', () => {
    expect(parseUnifiedDiff('')).toMatchObject({ hunks: [], additions: 0 })
    expect(parseUnifiedDiff('not a diff at all').preamble).toEqual(['not a diff at all'])
    // blank context lines whose leading space was stripped by a tool
    expect(parseUnifiedDiff('@@ -1,2 +1,2 @@\n\n-x\n+y\n').hunks[0]!.lines[0]).toMatchObject({
      kind: 'ctx',
      oldNo: 1,
      newNo: 1,
    })
  })
})

describe('diffLines', () => {
  it('finds an insertion, a deletion and a replacement', () => {
    const ops = diffLines(['a', 'b', 'c'], ['a', 'x', 'c', 'd'])
    expect(ops.map((o) => `${o.kind}:${o.text}`)).toEqual([
      'eq:a',
      'del:b',
      'add:x',
      'eq:c',
      'add:d',
    ])
    expect(diffStats(ops)).toEqual({ additions: 2, deletions: 1 })
  })

  it('numbers both sides', () => {
    const ops = diffLines(['a', 'b'], ['b'])
    expect(ops).toEqual([
      { kind: 'del', text: 'a', aNo: 1, bNo: null },
      { kind: 'eq', text: 'b', aNo: 2, bNo: 1 },
    ])
  })

  it('handles identical, empty and totally different inputs', () => {
    expect(diffLines(['a'], ['a']).every((o) => o.kind === 'eq')).toBe(true)
    expect(diffLines([], ['a']).map((o) => o.kind)).toEqual(['add'])
    expect(diffLines(['a'], []).map((o) => o.kind)).toEqual(['del'])
    expect(diffStats(diffLines(['1', '2'], ['3', '4']))).toEqual({ additions: 2, deletions: 2 })
  })

  it('diffs text and policy-like JSON', () => {
    const a = '{\n  "approvals": {\n    "min": 1\n  }\n}'
    const b = '{\n  "approvals": {\n    "min": 2\n  }\n}'
    const ops = diffText(a, b)
    expect(ops.filter((o) => o.kind !== 'eq').map((o) => o.text.trim())).toEqual([
      '"min": 1',
      '"min": 2',
    ])
  })
})
