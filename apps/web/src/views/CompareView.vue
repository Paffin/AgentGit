<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { DiffResponse } from '@/api/types'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { shortId } from '@/lib/format'
import { useBoard } from '@/stores/board'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DiffViewer from '@/ui/DiffViewer.vue'
import EmptyState from '@/ui/EmptyState.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextField from '@/ui/TextField.vue'

const { repo } = useRepoName()
const route = useRoute()
const router = useRouter()
const board = useBoard()
usePageTitle(() => `Compare · ${repo.value}`)

const q = (k: string) => (typeof route.query[k] === 'string' ? (route.query[k] as string) : '')
const from = ref(q('from'))
const to = ref(q('to'))
const result = ref<{ diff: DiffResponse; from: string; to: string } | null>(null)
const busy = ref(false)
const error = ref<ApiError | null>(null)
let serial = 0
let controller: AbortController | undefined
onBeforeUnmount(() => controller?.abort())
const isCurrent = (request: number, signal: AbortSignal) => request === serial && !signal.aborted

async function compare(repository: string, a: string, b: string, signal: AbortSignal) {
  // Resolve first so typos identify the unknown revision and results display immutable ids.
  const [fromId, toId] = await Promise.all([
    a ? api.history.resolve(repository, a, { signal }) : Promise.resolve(''),
    api.history.resolve(repository, b, { signal }),
  ])
  const diff = await api.tree.diff(repository, { from: fromId, to: toId, context: 3 }, { signal })
  return { diff, from: fromId, to: toId }
}

async function run() {
  controller?.abort()
  const current = new AbortController()
  controller = current
  const a = q('from')
  const b = q('to')
  const repository = repo.value
  if (!b) {
    serial++
    result.value = null
    error.value = null
    busy.value = false
    return
  }
  const mine = ++serial
  busy.value = true
  error.value = null
  try {
    const comparison = await compare(repository, a, b, current.signal)
    if (!isCurrent(mine, current.signal)) return
    result.value = comparison
  } catch (e) {
    if (!isCurrent(mine, current.signal)) return
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
    result.value = null
  } finally {
    if (mine === serial) busy.value = false
  }
}

function submit() {
  const a = from.value.trim()
  const b = to.value.trim()
  if (a === q('from') && b === q('to')) {
    void run()
    return
  }
  void router.push({ query: { from: a || undefined, to: b || undefined } })
}
function swap() {
  const oldFrom = from.value
  from.value = to.value
  to.value = oldFrom
  submit()
}
watch(
  [repo, () => route.query.from, () => route.query.to],
  () => {
    from.value = q('from')
    to.value = q('to')
    void run()
  },
  { immediate: true },
)

const names = computed(() => board.streams.map((s) => s.name))
</script>

<template>
  <div class="page">
    <div>
      <h1>Compare revisions</h1>
      <p class="muted">
        The diff between any two revisions. Leave “from” empty to compare against the empty tree.
      </p>
    </div>
    <Card>
      <form class="form" @submit.prevent="submit">
        <TextField
          v-model="from"
          label="From"
          mono
          placeholder="main~3, a changeset id, or empty"
          list="cmp-streams"
        />
        <TextField v-model="to" label="To" mono placeholder="main" list="cmp-streams" required />
        <Button type="submit" variant="primary" icon="diff" :loading="busy">Compare</Button>
        <Button :disabled="!from.trim() || !to.trim()" @click="swap">Swap revisions</Button>
        <datalist id="cmp-streams">
          <option v-for="s in names" :key="s" :value="s" />
        </datalist>
      </form>
    </Card>
    <Alert v-if="error" tone="bad" live="alert" title="Cannot compare">{{ error.message }}</Alert>
    <Skeleton v-if="busy && !result" :lines="5" />
    <p v-if="busy && result" class="muted" role="status">Updating comparison…</p>
    <template v-if="result">
      <p class="muted ids">
        <code class="mono">{{ result.from ? shortId(result.from, 12) : 'empty tree' }}</code>
        →
        <code class="mono">{{ shortId(result.to, 12) }}</code>
      </p>
      <DiffViewer
        :files="result.diff.files"
        :truncated="result.diff.truncated"
        anchor-prefix="cmp"
      />
    </template>
    <EmptyState
      v-else-if="!error && !busy"
      icon="diff"
      title="Pick two revisions"
      description="A branch name, a version id or prefix, a change request id, or an expression like main~2 or prop_…^2."
    />
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.form {
  display: grid;
  grid-template-columns: 1fr 1fr auto auto;
  gap: var(--s-3);
  align-items: end;
}
.ids {
  font-size: var(--fs-sm);
}
@media (max-width: 800px) {
  .form {
    grid-template-columns: 1fr;
  }
}
</style>
