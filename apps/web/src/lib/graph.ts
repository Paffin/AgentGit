// Lane assignment for drawing history as a graph (newest first). The classic algorithm: each lane waits for
// the commit it expects next; a commit takes the lane that waits for it, hands it to its first parent, and
// opens extra lanes for the other parents of a merge. State carries over between pages.

export interface GraphInput {
  id: string
  parents: readonly string[]
}

export interface GraphRow {
  id: string
  /** Lane of this commit's dot. */
  lane: number
  /** Which commit each lane was waiting for before this row (null = free). */
  above: (string | null)[]
  /** …and after it. */
  below: (string | null)[]
  /** Lanes that converged into this commit from above (including its own lane when it was waiting). */
  incoming: number[]
  /** For each parent, the lane in `below` where its line continues. */
  parentLanes: number[]
  isMerge: boolean
}

export interface GraphState {
  lanes: (string | null)[]
}

export const emptyGraphState = (): GraphState => ({ lanes: [] })

function firstFree(lanes: (string | null)[], from = 0): number {
  for (let i = from; i < lanes.length; i++) if (lanes[i] === null) return i
  lanes.push(null)
  return lanes.length - 1
}

export function buildGraph(
  commits: readonly GraphInput[],
  prior: GraphState = emptyGraphState(),
): { rows: GraphRow[]; state: GraphState; laneCount: number } {
  const lanes = prior.lanes.slice()
  const rows: GraphRow[] = []
  let laneCount = lanes.length
  for (const c of commits) {
    const above = lanes.slice()
    const incoming: number[] = []
    lanes.forEach((id, i) => {
      if (id === c.id) incoming.push(i)
    })
    const lane = incoming.length > 0 ? (incoming[0] as number) : firstFree(lanes)
    for (const i of incoming) lanes[i] = null

    const parentLanes: number[] = []
    c.parents.forEach((p, idx) => {
      const existing = lanes.indexOf(p)
      if (existing !== -1) {
        parentLanes.push(existing)
      } else if (idx === 0) {
        lanes[lane] = p
        parentLanes.push(lane)
      } else {
        const free = firstFree(lanes)
        lanes[free] = p
        parentLanes.push(free)
      }
    })
    // Trailing free lanes are dropped so the graph narrows again after a branch ends.
    while (lanes.length > 0 && lanes[lanes.length - 1] === null) lanes.pop()
    const below = lanes.slice()
    laneCount = Math.max(laneCount, above.length, below.length, lane + 1)
    rows.push({
      id: c.id,
      lane,
      above,
      below,
      incoming,
      parentLanes,
      isMerge: c.parents.length > 1,
    })
  }
  return { rows, state: { lanes }, laneCount }
}
