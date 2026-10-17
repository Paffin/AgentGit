<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { ReflogEntry, Stream } from '@/api/types'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { useResource } from '@/composables/useResource'
import { shortId } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import CopyButton from '@/ui/CopyButton.vue'
import DataTable from '@/ui/DataTable.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const { repo } = useRepoName()
const auth = useAuth()
const board = useBoard()
const toasts = useToasts()
usePageTitle(() => `Streams · ${repo.value}`)

const streams = useResource<Stream[]>(() => api.streams.list(repo.value), { watch: [repo] })
const selected = ref('')
watch(
  () => streams.data.value,
  (list) => {
    if (list && !list.some((s) => s.name === selected.value))
      selected.value = board.repo?.default_stream ?? list[0]?.name ?? ''
  },
  { immediate: true },
)
const reflog = useResource<ReflogEntry[]>(
  async () => (selected.value ? api.streams.reflog(repo.value, selected.value, 100) : []),
  { watch: [selected, repo] },
)

const streamColumns: Column[] = [
  { key: 'name', label: 'Stream', width: 'minmax(0, 1.4fr)' },
  { key: 'head', label: 'Head', width: 'minmax(0, 1.2fr)' },
  { key: 'version', label: 'Version', width: '6rem', align: 'end' },
  { key: 'policy_version', label: 'Policy', width: '6rem', align: 'end' },
  { key: 'created_by', label: 'Created by', width: 'minmax(0, 1.2fr)', hideBelow: 900 },
]
const logColumns: Column[] = [
  { key: 'version', label: 'Version', width: '5.5rem' },
  { key: 'change', label: 'Moved', width: 'minmax(0, 1.4fr)' },
  { key: 'reason', label: 'Why', width: 'minmax(0, 2fr)', hideBelow: 760 },
  { key: 'actor', label: 'By', width: 'minmax(0, 1.2fr)', hideBelow: 900 },
  { key: 'timestamp_ms', label: 'When', width: '7rem', align: 'end' },
]

// ---- create ---------------------------------------------------------------------------------------
const creating = ref(false)
const name = ref('')
const from = ref('')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const canCreate = computed(() => auth.hasRole(repo.value, 'writer'))

function openCreate() {
  name.value = ''
  from.value = ''
  error.value = null
  creating.value = true
}
async function create() {
  busy.value = true
  error.value = null
  try {
    await api.streams.create(repo.value, {
      name: name.value.trim(),
      from_rev: from.value.trim() || undefined,
    })
    creating.value = false
    toasts.ok('Stream created', name.value.trim())
    await streams.reload()
    board.touched(['streams'])
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>Streams</h1>
        <p class="muted">
          A stream is a named line of integration. Its head only moves by compare-and-swap, and
          every move is in the reflog.
        </p>
      </div>
      <Button
        icon="plus"
        :disabled="!canCreate"
        disabled-reason="Creating a stream requires the writer role"
        @click="openCreate"
        >New stream</Button
      >
    </div>

    <Card title="Streams" icon="branch" flush>
      <ErrorState
        v-if="streams.error.value && !streams.data.value"
        what="streams"
        :error="streams.error.value"
        @retry="streams.reload()"
      />
      <DataTable
        v-else
        :columns="streamColumns"
        :rows="streams.data.value ?? []"
        :row-key="(s: Stream) => s.name"
        caption="Streams"
        :loading="!streams.settled.value"
        :selected-key="selected"
      >
        <template #cell-name="{ row }">
          <button
            type="button"
            class="pick"
            :aria-pressed="selected === (row as Stream).name"
            @click="selected = (row as Stream).name"
          >
            <strong class="mono">{{ (row as Stream).name }}</strong>
          </button>
          <Badge v-if="(row as Stream).name === board.repo?.default_stream" size="sm" tone="accent"
            >default</Badge
          >
        </template>
        <template #cell-head="{ row }">
          <template v-if="(row as Stream).head">
            <RouterLink
              :to="{
                path: `/repos/${repo}/history`,
                query: { rev: (row as Stream).name },
              }"
              class="mono"
              >{{ shortId((row as Stream).head, 12) }}</RouterLink
            >
            <CopyButton :text="(row as Stream).head" label="Copy head id" />
          </template>
          <span v-else class="faint">empty</span>
        </template>
        <template #cell-created_by="{ row }"
          ><IdentityLabel :identity="(row as Stream).created_by" :size="18"
        /></template>
      </DataTable>
    </Card>

    <Card
      :title="`Reflog · ${selected || '–'}`"
      icon="history"
      subtitle="Append-only record of every head move"
      flush
    >
      <ErrorState
        v-if="reflog.error.value && !reflog.data.value"
        what="the reflog"
        :error="reflog.error.value"
        compact
        @retry="reflog.reload()"
      />
      <DataTable
        v-else
        :columns="logColumns"
        :rows="reflog.data.value ?? []"
        :row-key="(e: ReflogEntry) => String(e.seq)"
        caption="Reflog"
        :loading="!reflog.settled.value"
        dense
      >
        <template #cell-version="{ row }">v{{ (row as ReflogEntry).version }}</template>
        <template #cell-change="{ row }">
          <code class="mono faint">{{
            (row as ReflogEntry).old_head ? shortId((row as ReflogEntry).old_head, 8) : 'empty'
          }}</code>
          →
          <RouterLink
            :to="{
              path: `/repos/${repo}/history`,
              query: { rev: (row as ReflogEntry).new_head },
            }"
            class="mono"
            >{{ shortId((row as ReflogEntry).new_head, 8) }}</RouterLink
          >
        </template>
        <template #cell-reason="{ row }"
          ><span class="reason">{{ (row as ReflogEntry).reason }}</span></template
        >
        <template #cell-actor="{ row }"
          ><IdentityLabel :identity="(row as ReflogEntry).actor" :size="18"
        /></template>
        <template #cell-timestamp_ms="{ row }"
          ><span class="faint"><RelativeTime :ms="(row as ReflogEntry).timestamp_ms" /></span
        ></template>
        <template #empty>
          <EmptyState
            icon="history"
            compact
            title="Nothing has been integrated into this stream yet"
          />
        </template>
      </DataTable>
    </Card>

    <Dialog
      v-model="creating"
      title="New stream"
      description="Starts empty, or at a revision you name."
      :dismissable="!busy"
    >
      <form id="new-stream" class="form" @submit.prevent="create">
        <TextField
          v-model="name"
          label="Name"
          mono
          required
          placeholder="agents/refactor"
          autofocus
        />
        <TextField
          v-model="from"
          label="Start at (optional)"
          mono
          placeholder="main"
          hint="A stream, changeset id or other revision. Empty means an empty stream."
        />
        <Alert v-if="error" tone="bad" live="alert" title="Could not create the stream">{{
          error.message
        }}</Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="creating = false">Cancel</Button>
        <Button type="submit" form="new-stream" variant="primary" :loading="busy"
          >Create stream</Button
        >
      </template>
    </Dialog>
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
.pick {
  background: none;
  border: none;
  padding: 0;
  color: var(--text);
  margin-right: var(--s-2);
}
.pick:hover strong {
  color: var(--accent-text);
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.reason {
  color: var(--text-2);
  font-size: var(--fs-sm);
}
</style>
