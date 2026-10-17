// Layered layout for the task dependency graph. Pure data in, coordinates out: the component only draws.
import type { Task } from '@/api/types'

export type TaskStatus = 'done' | 'in_progress' | 'ready' | 'blocked' | 'cancelled'

export interface DagNode {
  id: string
  task: Task
  layer: number
  row: number
  x: number
  y: number
  status: TaskStatus
  /** Titles' ids of dependencies that are not done yet. */
  blockedBy: string[]
}

export interface DagEdge {
  from: string
  to: string
  path: string
  /** The dependency is not done: this edge is what holds `to` back. */
  holding: boolean
}

export interface DagLayout {
  nodes: DagNode[]
  edges: DagEdge[]
  width: number
  height: number
  counts: Record<TaskStatus, number>
  hiddenDone: number
}

export interface DagOptions {
  nodeW: number
  nodeH: number
  gapX: number
  gapY: number
  /** Done tasks nobody waits on are collapsed beyond this many (most recently updated first). */
  maxLooseDone: number
}

export const DEFAULT_DAG: DagOptions = {
  nodeW: 200,
  nodeH: 68,
  gapX: 44,
  gapY: 12,
  maxLooseDone: 3,
}

export function taskStatus(task: Task, byId: ReadonlyMap<string, Task>): TaskStatus {
  if (task.state === 'done') return 'done'
  if (task.state === 'cancelled') return 'cancelled'
  if (task.state === 'in_progress') return 'in_progress'
  const waiting = task.depends_on.some((d) => {
    const dep = byId.get(d)
    return dep ? dep.state !== 'done' : false
  })
  // The server also reports `ready`; trust it when it disagrees only for dependencies we cannot see.
  return waiting || !task.ready ? 'blocked' : 'ready'
}

const ORDER: Record<TaskStatus, number> = {
  in_progress: 0,
  ready: 1,
  blocked: 2,
  done: 3,
  cancelled: 4,
}

export function layoutDag(allTasks: readonly Task[], options: Partial<DagOptions> = {}): DagLayout {
  const o = { ...DEFAULT_DAG, ...options }
  const byId = new Map(allTasks.map((t) => [t.id, t]))

  // Which tasks to show: everything open, plus done tasks that still matter as context.
  const dependedOnByOpen = new Set<string>()
  for (const t of allTasks) {
    if (t.state === 'done' || t.state === 'cancelled') continue
    for (const d of t.depends_on) dependedOnByOpen.add(d)
  }
  const looseDone = allTasks
    .filter((t) => t.state === 'done' && !dependedOnByOpen.has(t.id))
    .sort((a, b) => b.updated_at_ms - a.updated_at_ms)
  const keepLoose = new Set(looseDone.slice(0, o.maxLooseDone).map((t) => t.id))
  const visible = allTasks.filter((t) => {
    if (t.state === 'cancelled') return false
    if (t.state === 'done') return dependedOnByOpen.has(t.id) || keepLoose.has(t.id)
    return true
  })
  const hiddenDone =
    allTasks.filter((t) => t.state === 'done').length -
    visible.filter((t) => t.state === 'done').length
  const inView = new Set(visible.map((t) => t.id))

  // Layer = length of the longest dependency chain below the task (cycle-safe).
  const layerOf = new Map<string, number>()
  const visiting = new Set<string>()
  const depth = (id: string): number => {
    const known = layerOf.get(id)
    if (known !== undefined) return known
    if (visiting.has(id)) return 0
    visiting.add(id)
    const t = byId.get(id)
    let d = 0
    for (const dep of t?.depends_on ?? []) if (inView.has(dep)) d = Math.max(d, depth(dep) + 1)
    visiting.delete(id)
    layerOf.set(id, d)
    return d
  }
  for (const t of visible) depth(t.id)

  const layers: Task[][] = []
  for (const t of visible) {
    const l = layerOf.get(t.id) ?? 0
    ;(layers[l] ??= []).push(t)
  }

  // Order rows: first layer by status/title, later layers by the mean row of their dependencies.
  const rowOf = new Map<string, number>()
  layers.forEach((layer, li) => {
    const keyed = layer.map((t) => {
      const deps = t.depends_on.filter((d) => inView.has(d) && rowOf.has(d))
      const bary = deps.length
        ? deps.reduce((s, d) => s + (rowOf.get(d) ?? 0), 0) / deps.length
        : Number.POSITIVE_INFINITY
      return { t, bary, status: ORDER[taskStatus(t, byId)] }
    })
    keyed.sort((a, b) =>
      li === 0 || (a.bary === Number.POSITIVE_INFINITY && b.bary === Number.POSITIVE_INFINITY)
        ? a.status - b.status || a.t.title.localeCompare(b.t.title) || a.t.id.localeCompare(b.t.id)
        : a.bary - b.bary || a.t.title.localeCompare(b.t.title) || a.t.id.localeCompare(b.t.id),
    )
    layers[li] = keyed.map((k) => k.t)
    layers[li]?.forEach((t, row) => rowOf.set(t.id, row))
  })

  const counts: Record<TaskStatus, number> = {
    done: 0,
    in_progress: 0,
    ready: 0,
    blocked: 0,
    cancelled: 0,
  }
  for (const t of allTasks) counts[taskStatus(t, byId)]++

  const nodes: DagNode[] = []
  const nodeById = new Map<string, DagNode>()
  layers.forEach((layer, li) => {
    layer.forEach((t, row) => {
      const status = taskStatus(t, byId)
      const node: DagNode = {
        id: t.id,
        task: t,
        layer: li,
        row,
        x: li * (o.nodeW + o.gapX),
        y: row * (o.nodeH + o.gapY),
        status,
        blockedBy: t.depends_on.filter((d) => byId.get(d)?.state !== 'done'),
      }
      nodes.push(node)
      nodeById.set(t.id, node)
    })
  })

  const edges: DagEdge[] = []
  for (const n of nodes) {
    for (const dep of n.task.depends_on) {
      const from = nodeById.get(dep)
      if (!from) continue
      const x1 = from.x + o.nodeW
      const y1 = from.y + o.nodeH / 2
      const x2 = n.x
      const y2 = n.y + o.nodeH / 2
      const mid = (x1 + x2) / 2
      edges.push({
        from: dep,
        to: n.id,
        path: `M${x1} ${y1} C${mid} ${y1} ${mid} ${y2} ${x2} ${y2}`,
        holding: byId.get(dep)?.state !== 'done',
      })
    }
  }

  const width = layers.length === 0 ? 0 : layers.length * o.nodeW + (layers.length - 1) * o.gapX
  const rows = layers.reduce((m, l) => Math.max(m, l.length), 0)
  const height = rows === 0 ? 0 : rows * o.nodeH + (rows - 1) * o.gapY
  return { nodes, edges, width, height, counts, hiddenDone }
}
