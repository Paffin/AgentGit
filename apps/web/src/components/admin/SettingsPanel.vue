<script setup lang="ts">
import { ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Quota, Repo } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { formatBytes } from '@/lib/format'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import ErrorState from '@/ui/ErrorState.vue'
import KeyValue from '@/ui/KeyValue.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'
import type { KvItem } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const toasts = useToasts()
const board = useBoard()

const info = useResource<Repo>(() => api.repos.get(props.repo), { watch: [() => props.repo] })

const description = ref('')
const q = ref<Record<keyof Quota, string>>({
  max_total_object_bytes: '0',
  max_object_bytes: '0',
  max_streams: '0',
  max_open_proposals: '0',
  requests_per_second: '0',
  burst: '0',
})
watch(
  () => info.data.value,
  (r) => {
    if (!r) return
    description.value = r.description
    for (const k of Object.keys(q.value) as (keyof Quota)[]) q.value[k] = String(r.quota?.[k] ?? 0)
  },
  { immediate: true },
)

const FIELDS: { key: keyof Quota; label: string; hint: string }[] = [
  { key: 'max_total_object_bytes', label: 'Total storage (bytes)', hint: '0 = unlimited' },
  { key: 'max_object_bytes', label: 'Largest object (bytes)', hint: '0 = server default' },
  { key: 'max_streams', label: 'Streams', hint: '0 = unlimited' },
  { key: 'max_open_proposals', label: 'Open proposals', hint: '0 = unlimited' },
  { key: 'requests_per_second', label: 'Requests per second', hint: '0 = server default' },
  { key: 'burst', label: 'Burst', hint: '0 = server default' },
]

const busy = ref(false)
const error = ref<ApiError | null>(null)
async function save() {
  busy.value = true
  error.value = null
  try {
    const quota: Partial<Quota> = {}
    for (const f of FIELDS) {
      const n = Number(q.value[f.key])
      if (!Number.isInteger(n) || n < 0)
        throw new ApiError({
          status: 0,
          code: 'invalid_argument',
          message: `${f.label} must be a whole number, 0 or more.`,
        })
      quota[f.key] = n
    }
    await api.repos.configure(props.repo, { description: description.value, quota })
    toasts.ok('Settings saved')
    await info.reload()
    board.refreshAll()
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}

function kv(r: Repo): KvItem[] {
  return [
    { key: 'ns', label: 'Namespace', value: r.namespace, mono: true },
    { key: 'meta', label: 'Metadata store', value: r.meta_backend },
    { key: 'obj', label: 'Object store', value: r.object_backend },
    { key: 'dur', label: 'Durability', value: r.durability },
    {
      key: 'counts',
      label: 'Contents',
      value: `${r.stream_count} streams, ${r.changeset_count} changesets`,
    },
    { key: 'by', label: 'Created by', value: r.created_by },
  ]
}
</script>

<template>
  <div class="settings">
    <ErrorState
      v-if="info.error.value && !info.data.value"
      what="the repository"
      :error="info.error.value"
      compact
      @retry="info.reload()"
    />
    <template v-else-if="info.data.value">
      <Card title="About this repository" icon="database">
        <KeyValue :items="kv(info.data.value)" label-width="10rem" />
      </Card>
      <Card
        title="Settings and quota"
        icon="sliders"
        subtitle="Limits are enforced by the server: a write over the quota is refused with quota_exceeded."
      >
        <form class="form" @submit.prevent="save">
          <TextArea v-model="description" label="Description" :rows="2" />
          <div class="grid">
            <TextField
              v-for="f in FIELDS"
              :key="f.key"
              v-model="q[f.key]"
              :label="f.label"
              type="number"
              :min="0"
              :hint="
                f.hint +
                (f.key.includes('bytes') && Number(q[f.key]) > 0
                  ? ` · ${formatBytes(Number(q[f.key]))}`
                  : '')
              "
              mono
            />
          </div>
          <Alert v-if="error" tone="bad" live="alert" title="Could not save">{{
            error.message
          }}</Alert>
          <div>
            <Button type="submit" variant="primary" :loading="busy">Save settings</Button>
          </div>
        </form>
      </Card>
    </template>
  </div>
</template>

<style scoped>
.settings {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-3) var(--s-4);
}
</style>
