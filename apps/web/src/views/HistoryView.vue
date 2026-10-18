<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import type { Changeset } from '@/api/types'
import ChangesetDetail from '@/components/history/ChangesetDetail.vue'
import GraphCell from '@/components/history/GraphCell.vue'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { firstLine, shortId } from '@/lib/format'
import { buildGraph, emptyGraphState } from '@/lib/graph'
import type { GraphRow, GraphState } from '@/lib/graph'
import { useBoard } from '@/stores/board'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import LoadMore from '@/ui/LoadMore.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextField from '@/ui/TextField.vue'

const { repo } = useRepoName()
const route = useRoute()
const router = useRouter()
const board = useBoard()
usePageTitle(() => `History · ${repo.value}`)

const rev = ref(typeof route.query.rev === 'string' ? route.query.rev : '')
const draft = ref(rev.value)
const path = ref(typeof route.query.path === 'string' ? route.query.path : '')
const selected = computed(() => (typeof route.query.cs === 'string' ? route.query.cs : ''))

const rows = ref<{ cs: Changeset; graph: GraphRow }[]>([])
const lanes = ref(1)
const next = ref('')
const loading = ref(true)
const more = ref(false)
const error = ref<unknown>(null)
const empty = ref(false)
let state: GraphState = emptyGraphState()
let serial = 0

// The repository's own default stream; until its record has loaded there is nothing to ask for.
const effectiveRev = computed(() => rev.value || board.repo?.default_stream || '')

async function load(append = false) {
  const mine = ++serial
  if (!effectiveRev.value) {
    if (board.load.repo.status === 'error') {
      error.value = board.load.repo.error
      loading.value = false
    }
    return
  }
  if (append) more.value = true
  else {
    loading.value = true
    state = emptyGraphState()
  }
  error.value = null
  empty.value = false
  try {
    const page = await api.history.log(repo.value, {
      rev: effectiveRev.value,
      path: path.value || undefined,
      page_size: 50,
      page_token: append ? next.value : undefined,
    })
    if (mine !== serial) return
    const built = buildGraph(
      page.items.map((c) => ({ id: c.id, parents: c.parents })),
      state,
    )
    state = built.state
    const add = page.items.map((cs, i) => ({ cs, graph: built.rows[i] as GraphRow }))
    rows.value = append ? [...rows.value, ...add] : add
    lanes.value = Math.max(append ? lanes.value : 1, built.laneCount)
    next.value = page.nextPageToken
  } catch (e) {
    if (mine !== serial) return
    error.value = e
  } finally {
    if (mine === serial) {
      loading.value = false
      more.value = false
    }
  }
}

watch(
  [() => board.repo?.default_stream, () => board.load.repo.status, rev, path, repo],
  () => void load(),
  { immediate: true },
)

function apply() {
  rev.value = draft.value.trim()
  void router.replace({
    query: {
      ...route.query,
      rev: rev.value || undefined,
      path: path.value || undefined,
      cs: undefined,
    },
  })
}
function clearPath() {
  path.value = ''
  void router.replace({ query: { ...route.query, path: undefined } })
}
function select(id: string) {
  void router.replace({ query: { ...route.query, cs: id } })
}
const streamNames = computed(() => board.streams.map((s) => s.name))
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>History</h1>
        <p class="muted">
          Newest first. Merge commits show their parents. Open a file for line-by-line blame.
        </p>
      </div>
      <form class="rev" @submit.prevent="apply">
        <TextField
          v-model="draft"
          label="Revision"
          mono
          :placeholder="effectiveRev"
          hint="A stream, changeset id, proposal id, or main~2"
          list="streams-list"
        />
        <Button type="submit" icon="search">Show</Button>
        <datalist id="streams-list">
          <option v-for="s in streamNames" :key="s" :value="s" />
        </datalist>
      </form>
    </div>
    <p v-if="path" class="filter">
      <Badge mono icon="file">{{ path }}</Badge>
      <button type="button" class="clear" @click="clearPath">Clear path filter</button>
    </p>

    <div class="layout" :class="{ split: !!selected }">
      <Card :title="`Changesets · ${effectiveRev}`" icon="history" flush>
        <ErrorState v-if="error" what="history" :error="error" @retry="load()" />
        <div v-else-if="loading" class="sk">
          <Skeleton :height="40" /><Skeleton :height="40" /><Skeleton :height="40" />
        </div>
        <EmptyState
          v-else-if="empty || rows.length === 0"
          icon="history"
          title="No history yet"
          description="Nothing has been integrated into this stream."
        />
        <template v-else>
          <ol class="log" aria-label="Changesets, newest first" data-testid="history-log">
            <li v-for="r in rows" :key="r.cs.id" class="row" :class="{ sel: r.cs.id === selected }">
              <GraphCell :row="r.graph" :lanes="lanes" />
              <button
                type="button"
                class="main"
                :aria-current="r.cs.id === selected ? 'true' : undefined"
                @click="select(r.cs.id)"
              >
                <span class="m">{{ firstLine(r.cs.message) || '(no message)' }}</span>
                <span class="s">
                  <IdentityLabel :identity="r.cs.author" :size="16" />
                  <RelativeTime :ms="r.cs.timestamp_ms" class="faint" />
                  <Badge v-if="r.cs.parents.length > 1" size="sm" tone="accent" icon="merge"
                    >merge</Badge
                  >
                </span>
              </button>
              <code class="id mono faint">{{ shortId(r.cs.id, 8) }}</code>
            </li>
          </ol>
          <LoadMore
            :has-more="!!next"
            :loading="more"
            :shown="rows.length"
            noun="changesets"
            @more="load(true)"
          />
        </template>
      </Card>
      <ChangesetDetail v-if="selected" :id="selected" :repo="repo" @select="select" />
    </div>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.head {
  display: flex;
  justify-content: space-between;
  align-items: flex-end;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.rev {
  display: flex;
  gap: var(--s-2);
  align-items: flex-end;
}
.rev :deep(.field) {
  width: 300px;
  max-width: 60vw;
}
.filter {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}
.clear {
  background: none;
  border: none;
  color: var(--accent-text);
  text-decoration: underline;
}
.layout {
  display: grid;
  gap: var(--s-4);
  grid-template-columns: minmax(0, 1fr);
  align-items: start;
}
.layout.split {
  grid-template-columns: minmax(0, 1fr);
}
@media (min-width: 1280px) {
  .layout.split {
    grid-template-columns: minmax(0, 5fr) minmax(0, 6fr);
  }
}
.sk {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  padding: var(--s-4);
}
.log {
  list-style: none;
  margin: 0;
  padding: 0;
}
.row {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  padding-right: var(--s-3);
  border-bottom: 1px solid var(--border);
  min-height: 44px;
}
.row.sel {
  background: var(--accent-soft);
}
.row:hover {
  background: var(--surface-2);
}
.main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  text-align: left;
  background: none;
  border: none;
  padding: 4px 0;
  color: var(--text);
}
.m {
  font-weight: 560;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.s {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  font-size: var(--fs-xs);
}
.id {
  font-size: var(--fs-xs);
}
</style>
