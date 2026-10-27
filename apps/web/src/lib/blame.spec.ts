// @vitest-environment node
import { describe, expect, it } from 'vitest'
import type { BlameLine } from '@/api/types'
import { groupBlame } from './blame'

const line = (n: number, changeset: string, extra: Partial<BlameLine> = {}): BlameLine => ({
  line: n,
  original_line: n,
  changeset,
  author: 'user:alice',
  summary: `change ${changeset}`,
  text: `line ${n}`,
  timestamp_ms: 1000,
  path: 'src/a.ts',
  boundary: false,
  ...extra,
})

describe('groupBlame', () => {
  it('returns nothing for nothing', () => {
    expect(groupBlame([])).toEqual([])
  })

  it('merges consecutive lines of one changeset into a run', () => {
    const runs = groupBlame([line(1, 'a'), line(2, 'a'), line(3, 'b'), line(4, 'a')])
    expect(runs.map((r) => [r.changeset, r.lines.length])).toEqual([
      ['a', 2],
      ['b', 1],
      ['a', 1],
    ])
  })

  it('starts a new run when the file had another name or the origin is only a boundary', () => {
    const runs = groupBlame([
      line(1, 'a'),
      line(2, 'a', { path: 'src/old-name.ts' }),
      line(3, 'a', { path: 'src/old-name.ts', boundary: true }),
    ])
    expect(runs).toHaveLength(3)
    expect(runs[1]!.path).toBe('src/old-name.ts')
    expect(runs[2]!.boundary).toBe(true)
  })

  it('keeps the metadata of the first line of the run', () => {
    const [run] = groupBlame([
      line(1, 'a', { author: 'agent:bot#run_1', timestamp_ms: 5 }),
      line(2, 'a'),
    ])
    expect(run!.author).toBe('agent:bot#run_1')
    expect(run!.timestampMs).toBe(5)
    expect(run!.summary).toBe('change a')
  })
})
