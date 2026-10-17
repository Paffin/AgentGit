<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { Overlap } from '@/api/types'
import { shortId } from '@/lib/format'
import { useBoard } from '@/stores/board'
import Badge from '@/ui/Badge.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{ repo: string }>()
const board = useBoard()

interface Ref {
  kind: string
  id: string
  label: string
  to?: string
}

function parseRef(raw: string): Ref {
  const at = raw.indexOf(':')
  const kind = at === -1 ? 'other' : raw.slice(0, at)
  const id = at === -1 ? raw : raw.slice(at + 1)
  if (kind === 'proposal') {
    const p = [...board.proposals, ...board.recent].find((x) => x.id === id)
    return {
      kind,
      id,
      label: p?.title || shortId(id, 10),
      to: `/repos/${props.repo}/proposals/${encodeURIComponent(id)}`,
    }
  }
  if (kind === 'task') return { kind, id, label: board.taskById.get(id)?.title ?? shortId(id, 10) }
  if (kind === 'lease') {
    const l = board.leases.find((x) => x.id === id)
    return { kind, id, label: l ? `lease on ${l.scope}` : shortId(id, 10) }
  }
  if (kind === 'stream') return { kind, id, label: `stream ${id}` }
  return { kind, id, label: raw }
}

const SENTENCE: Record<string, (a: string, b: string) => string> = {
  proposals_touch_same_paths: () => 'edit the same files',
  stream_moved_under_proposal: () => 'changed files this proposal also edits since it was opened',
  proposal_inside_foreign_lease: () => 'edits files inside a lease held by someone else',
  leases_overlap: () => 'may cover the same paths',
  task_scopes_overlap: () => 'have overlapping scopes',
}

const rows = computed(() =>
  board.overlaps.map((o: Overlap) => ({
    key: `${o.a}|${o.b}|${o.kind}`,
    a: parseRef(o.a),
    b: parseRef(o.b),
    what: SENTENCE[o.kind]?.(o.a, o.b) ?? o.kind.replace(/_/g, ' '),
    paths: o.paths,
  })),
)
</script>

<template>
  <Card
    title="Conflict predictions"
    icon="alert"
    :subtitle="rows.length ? `${rows.length} overlaps` : 'path-level early warning'"
    flush
  >
    <div class="body" data-testid="overlap-panel">
      <div v-if="board.load.overlaps.status === 'loading' && rows.length === 0" class="sk">
        <Skeleton :height="40" />
      </div>
      <ErrorState
        v-else-if="board.load.overlaps.status === 'error' && rows.length === 0"
        what="predictions"
        :error="board.load.overlaps.error"
        compact
        @retry="board.refreshAll()"
      />
      <EmptyState
        v-else-if="rows.length === 0"
        icon="check"
        compact
        title="No overlapping work"
        description="Nobody is editing the same paths. This is a path-level warning; the definitive answer is each proposal’s Mergeable state."
      />
      <ul v-else class="list" role="list">
        <li v-for="r in rows" :key="r.key" class="ov">
          <p class="sentence">
            <component :is="r.a.to ? RouterLink : 'span'" :to="r.a.to" class="ref">{{
              r.a.label
            }}</component>
            <Icon name="link" :size="13" class="and" />
            <component :is="r.b.to ? RouterLink : 'span'" :to="r.b.to" class="ref">{{
              r.b.label
            }}</component>
            <span class="what">{{ r.what }}</span>
          </p>
          <p class="paths">
            <Badge v-for="p in r.paths.slice(0, 5)" :key="p" mono size="sm" tone="warn">{{
              p
            }}</Badge>
            <span v-if="r.paths.length > 5" class="faint">and {{ r.paths.length - 5 }} more</span>
          </p>
        </li>
      </ul>
    </div>
  </Card>
</template>

<style scoped>
.body {
  min-height: 110px;
}
.sk {
  padding: var(--s-4);
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
}
.ov {
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.sentence {
  display: flex;
  flex-wrap: wrap;
  align-items: baseline;
  gap: 4px 8px;
  font-size: var(--fs-sm);
}
.ref {
  font-weight: 620;
  color: var(--text);
}
a.ref {
  color: var(--accent-text);
}
.and {
  color: var(--text-3);
  align-self: center;
}
.what {
  color: var(--text-2);
}
.paths {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  font-size: var(--fs-xs);
}
</style>
