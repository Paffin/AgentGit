// @vitest-environment node
import { describe, expect, it } from 'vitest'
import type { Task } from '@/api/types'
import { DEFAULT_DAG, layoutDag, taskStatus } from './dag'
import { buildGraph } from './graph'

let n = 0
const task = (id: string, over: Partial<Task> = {}): Task => ({
  id,
  title: id,
  description: '',
  owner: '',
  state: 'open',
  scope: [],
  created_by: 'user:a',
  created_at_ms: ++n,
  updated_at_ms: n,
  depends_on: [],
  ready: true,
  max_attempts: 0,
  attempts_started: 0,
  ...over,
})

describe('task graph layout', () => {
  const tasks = [
    task('design', { state: 'done' }),
    task('api', { depends_on: ['design'], state: 'in_progress', owner: 'agent:bot' }),
    task('ui', { depends_on: ['api'], ready: false }),
    task('docs', { depends_on: ['api', 'design'], ready: false }),
    task('lint'),
  ]

  it('classifies tasks', () => {
    const byId = new Map(tasks.map((t) => [t.id, t]))
    expect(taskStatus(tasks[0]!, byId)).toBe('done')
    expect(taskStatus(tasks[1]!, byId)).toBe('in_progress')
    expect(taskStatus(tasks[2]!, byId)).toBe('blocked')
    expect(taskStatus(tasks[4]!, byId)).toBe('ready')
    expect(taskStatus(task('x', { state: 'cancelled' }), byId)).toBe('cancelled')
  })

  it('puts a task one layer to the right of its deepest dependency', () => {
    const l = layoutDag(tasks)
    const layer = (id: string) => l.nodes.find((x) => x.id === id)!.layer
    expect(layer('design')).toBe(0)
    expect(layer('api')).toBe(1)
    expect(layer('ui')).toBe(2)
    expect(layer('docs')).toBe(2)
    expect(layer('lint')).toBe(0)
    expect(l.width).toBe(3 * DEFAULT_DAG.nodeW + 2 * DEFAULT_DAG.gapX)
  })

  it('draws an edge from every dependency, marking the ones that still hold a task back', () => {
    const l = layoutDag(tasks)
    expect(l.edges).toHaveLength(4)
    const holding = l.edges.filter((e) => e.holding).map((e) => `${e.from}>${e.to}`)
    expect(holding.sort()).toEqual(['api>docs', 'api>ui'])
    expect(l.edges[0]!.path).toMatch(/^M[\d.]+ [\d.]+ C/)
  })

  it('counts statuses over all tasks and never overlaps nodes', () => {
    const l = layoutDag(tasks)
    expect(l.counts).toMatchObject({ done: 1, in_progress: 1, blocked: 2, ready: 1 })
    const keys = new Set(l.nodes.map((x) => `${x.x},${x.y}`))
    expect(keys.size).toBe(l.nodes.length)
  })

  it('collapses finished tasks nobody waits on, and hides cancelled ones', () => {
    const many = [
      ...Array.from({ length: 6 }, (_, i) => task(`old${i}`, { state: 'done', updated_at_ms: i })),
      task('gone', { state: 'cancelled' }),
      task('open'),
    ]
    const l = layoutDag(many, { maxLooseDone: 2 })
    expect(l.nodes.map((x) => x.id)).toContain('open')
    expect(l.nodes.map((x) => x.id)).not.toContain('gone')
    expect(l.nodes.filter((x) => x.status === 'done')).toHaveLength(2)
    expect(l.hiddenDone).toBe(4)
  })

  it('survives a dependency cycle and an unknown dependency', () => {
    const l = layoutDag([
      task('a', { depends_on: ['b'] }),
      task('b', { depends_on: ['a'] }),
      task('c', { depends_on: ['missing'] }),
    ])
    expect(l.nodes).toHaveLength(3)
  })

  it('is empty for no tasks', () => {
    expect(layoutDag([])).toMatchObject({ nodes: [], edges: [], width: 0, height: 0 })
  })
})

describe('commit graph lanes', () => {
  it('keeps a linear history in one lane', () => {
    const { rows, laneCount } = buildGraph([
      { id: 'c', parents: ['b'] },
      { id: 'b', parents: ['a'] },
      { id: 'a', parents: [] },
    ])
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0])
    expect(laneCount).toBe(1)
    expect(rows[2]!.parentLanes).toEqual([])
  })

  it('draws a merge with a second lane that rejoins', () => {
    // m merges main (b) and a side branch (s); both descend from a.
    const { rows, laneCount } = buildGraph([
      { id: 'm', parents: ['b', 's'] },
      { id: 'b', parents: ['a'] },
      { id: 's', parents: ['a'] },
      { id: 'a', parents: [] },
    ])
    expect(rows[0]).toMatchObject({ lane: 0, isMerge: true, parentLanes: [0, 1] })
    expect(rows[1]!.lane).toBe(0)
    expect(rows[2]!.lane).toBe(1)
    // the side branch's line bends into the lane that already waits for the common ancestor
    expect(rows[2]!.parentLanes).toEqual([0])
    expect(rows[2]!.below).toEqual(['a'])
    expect(rows[3]!.incoming).toEqual([0])
    expect(rows[3]!.lane).toBe(0)
    expect(rows[3]!.below).toEqual([])
    expect(laneCount).toBe(2)
  })

  it('opens a new lane for a head nothing points at yet', () => {
    const { rows } = buildGraph([
      { id: 'x', parents: ['a'] },
      { id: 'y', parents: ['a'] },
      { id: 'a', parents: [] },
    ])
    expect(rows[0]!.lane).toBe(0)
    expect(rows[1]!.lane).toBe(1)
  })

  it('continues across pages', () => {
    const first = buildGraph([{ id: 'c', parents: ['b'] }])
    const second = buildGraph([{ id: 'b', parents: [] }], first.state)
    expect(second.rows[0]!.incoming).toEqual([0])
    expect(second.rows[0]!.lane).toBe(0)
  })
})
