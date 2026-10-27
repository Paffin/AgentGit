<script setup lang="ts">
import { computed, ref } from 'vue'
import { DEFAULT_DAG } from '@/lib/dag'
import type { DagNode, TaskStatus } from '@/lib/dag'
import { useBoard } from '@/stores/board'
import Badge from '@/ui/Badge.vue'
import Card from '@/ui/Card.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import KeyValue from '@/ui/KeyValue.vue'
import type { IconName } from '@/ui/icons'
import Skeleton from '@/ui/Skeleton.vue'
import type { KvItem } from '@/ui/types'
import RelativeTime from '@/ui/RelativeTime.vue'

const board = useBoard()
const layout = computed(() => board.taskLayout)
const { nodeW, nodeH } = DEFAULT_DAG

const LABEL: Record<TaskStatus, string> = {
  done: 'Done',
  in_progress: 'In progress',
  ready: 'Ready',
  blocked: 'Blocked',
  cancelled: 'Cancelled',
}
const ICON: Record<TaskStatus, IconName> = {
  done: 'check',
  in_progress: 'play',
  ready: 'circle-dot',
  blocked: 'lock',
  cancelled: 'x',
}

function blockerNames(n: DagNode): string {
  const blockers = n.blockedBy.map((id) => board.taskById.get(id)?.title ?? id)
  return blockers.join(', ')
}

const selected = ref<DagNode | null>(null)
const open = computed({
  get: () => selected.value !== null,
  set: (v: boolean) => {
    if (!v) selected.value = null
  },
})
const details = computed<KvItem[]>(() => {
  const t = selected.value?.task
  if (!t) return []
  return [
    { key: 'state', label: 'State' },
    { key: 'owner', label: 'Owner' },
    { key: 'scope', label: 'Scope' },
    { key: 'deps', label: 'Waiting for' },
    { key: 'created', label: 'Created' },
    { key: 'id', label: 'Id', value: t.id, mono: true },
  ]
})
const summary = computed(() => {
  const c = layout.value.counts
  return `${c.ready} ready · ${c.blocked} blocked · ${c.in_progress} in progress · ${c.done} done`
})
</script>

<template>
  <Card title="Tasks and dependencies" icon="task" :subtitle="summary" flush>
    <div class="body" data-testid="task-graph">
      <div v-if="board.load.tasks.status === 'loading' && layout.nodes.length === 0" class="sk">
        <Skeleton :height="68" width="200px" /><Skeleton :height="68" width="200px" />
      </div>
      <ErrorState
        v-else-if="board.load.tasks.status === 'error' && layout.nodes.length === 0"
        what="tasks"
        :error="board.load.tasks.error"
        compact
        @retry="board.refreshAll()"
      />
      <EmptyState
        v-else-if="layout.nodes.length === 0"
        icon="task"
        compact
        title="No open tasks"
        description="Tasks give agents ownership of a piece of work; dependencies decide what is ready."
      />
      <div
        v-else
        class="scroller"
        tabindex="0"
        role="group"
        aria-label="Task dependency graph, scrollable"
      >
        <div class="canvas" :style="{ width: `${layout.width}px`, height: `${layout.height}px` }">
          <svg
            class="edges"
            :width="layout.width"
            :height="layout.height"
            aria-hidden="true"
            focusable="false"
          >
            <path
              v-for="e in layout.edges"
              :key="`${e.from}>${e.to}`"
              :d="e.path"
              :class="{ holding: e.holding }"
            />
          </svg>
          <ul class="nodes" role="list" aria-label="Tasks">
            <li
              v-for="n in layout.nodes"
              :key="n.id"
              class="node-wrap"
              :style="{
                left: `${n.x}px`,
                top: `${n.y}px`,
                width: `${nodeW}px`,
                height: `${nodeH}px`,
              }"
            >
              <button
                type="button"
                class="node"
                :class="`s-${n.status}`"
                :data-task="n.id"
                @click="selected = n"
              >
                <span class="row1">
                  <Icon :name="ICON[n.status]" :size="13" />
                  <span class="state">{{ LABEL[n.status] }}</span>
                  <span v-if="n.blockedBy.length" class="by"
                    >· {{ n.blockedBy.length }} to wait</span
                  >
                </span>
                <span class="title">{{ n.task.title }}</span>
                <span v-if="n.blockedBy.length" class="sr-only">
                  waiting for {{ blockerNames(n) }}
                </span>
              </button>
            </li>
          </ul>
        </div>
      </div>
      <p v-if="layout.hiddenDone > 0" class="hidden faint">
        {{ layout.hiddenDone }} finished
        {{ layout.hiddenDone === 1 ? 'task is' : 'tasks are' }} collapsed.
      </p>
    </div>

    <Dialog v-model="open" :title="selected?.task.title ?? ''" size="md">
      <template v-if="selected">
        <KeyValue :items="details" label-width="7.5rem">
          <template #value-state>
            <Badge
              :tone="
                selected.status === 'done'
                  ? 'ok'
                  : selected.status === 'blocked'
                    ? 'warn'
                    : selected.status === 'in_progress'
                      ? 'info'
                      : 'neutral'
              "
            >
              {{ LABEL[selected.status] }}
            </Badge>
          </template>
          <template #value-owner>
            <IdentityLabel v-if="selected.task.owner" :identity="selected.task.owner" />
            <span v-else class="faint">nobody yet</span>
          </template>
          <template #value-scope>
            <span v-if="selected.task.scope.length === 0" class="faint">whole repository</span>
            <span v-else class="mono">{{ selected.task.scope.join(', ') }}</span>
          </template>
          <template #value-deps>
            <span v-if="selected.task.depends_on.length === 0" class="faint">nothing</span>
            <ul v-else class="deps" role="list">
              <li v-for="d in selected.task.depends_on" :key="d">
                {{ board.taskById.get(d)?.title ?? d }}
                <Badge size="sm" :tone="board.taskById.get(d)?.state === 'done' ? 'ok' : 'warn'">{{
                  board.taskById.get(d)?.state ?? 'unknown'
                }}</Badge>
              </li>
            </ul>
          </template>
          <template #value-created>
            <RelativeTime :ms="selected.task.created_at_ms" /> by
            {{ selected.task.created_by }}
          </template>
        </KeyValue>
        <p v-if="selected.task.description" class="desc">{{ selected.task.description }}</p>
      </template>
    </Dialog>
  </Card>
</template>

<style scoped>
.body {
  min-height: 150px;
}
.sk {
  padding: var(--s-4);
  display: flex;
  gap: var(--s-3);
}
.scroller {
  overflow: auto;
  padding: var(--s-4);
}
.canvas {
  position: relative;
}
.edges {
  position: absolute;
  inset: 0;
  overflow: visible;
}
.edges path {
  fill: none;
  stroke: var(--border-strong);
  stroke-width: 1.5;
  opacity: 0.55;
}
.edges path.holding {
  stroke: var(--warn);
  opacity: 0.9;
  stroke-dasharray: 5 4;
}
.nodes {
  list-style: none;
  margin: 0;
  padding: 0;
}
.node-wrap {
  position: absolute;
}
.node {
  width: 100%;
  height: 100%;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 3px;
  padding: 6px 10px;
  text-align: left;
  background: var(--surface);
  border: 1px solid var(--border-strong);
  border-left-width: 4px;
  border-radius: var(--r-sm);
  color: var(--text);
  transition:
    transform var(--dur-1) var(--ease),
    box-shadow var(--dur-1) var(--ease);
}
.node:hover {
  transform: translateY(-1px);
  box-shadow: var(--shadow-2);
}
.row1 {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 0.6875rem;
  font-weight: 650;
  text-transform: uppercase;
  letter-spacing: 0.03em;
  color: var(--n-fg);
}
.by {
  text-transform: none;
  font-weight: 500;
  letter-spacing: 0;
  color: var(--text-3);
}
.title {
  font-size: var(--fs-sm);
  font-weight: 560;
  line-height: 1.25;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.s-done {
  --n-fg: var(--ok);
  border-left-color: var(--ok);
  background: color-mix(in srgb, var(--ok-soft) 55%, var(--surface));
}
.s-in_progress {
  --n-fg: var(--info);
  border-left-color: var(--info);
  background: color-mix(in srgb, var(--info-soft) 55%, var(--surface));
}
.s-ready {
  --n-fg: var(--accent-text);
  border-left-color: var(--accent);
}
.s-blocked {
  --n-fg: var(--warn);
  border-left-color: var(--warn);
  background: color-mix(in srgb, var(--warn-soft) 45%, var(--surface));
}
.s-cancelled {
  --n-fg: var(--text-3);
  opacity: 0.6;
}
.hidden {
  padding: 0 var(--s-4) var(--s-3);
  font-size: var(--fs-xs);
}
.deps {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.desc {
  margin-top: var(--s-3);
  padding-top: var(--s-3);
  border-top: 1px solid var(--border);
  color: var(--text-2);
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
@media (prefers-reduced-motion: reduce) {
  .node {
    transition: none;
  }
  .node:hover {
    transform: none;
  }
}
</style>
