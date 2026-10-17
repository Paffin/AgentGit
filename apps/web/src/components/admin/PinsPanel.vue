<script setup lang="ts">
import { ref } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Pin } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { shortId } from '@/lib/format'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const toasts = useToasts()
const pins = useResource<Pin[]>(() => api.admin.pins(props.repo), { watch: [() => props.repo] })

const name = ref('')
const rev = ref('')
const reason = ref('')
const busy = ref(false)
const error = ref<ApiError | null>(null)

async function add() {
  busy.value = true
  error.value = null
  try {
    await api.admin.pin(props.repo, {
      name: name.value.trim(),
      rev: rev.value.trim(),
      reason: reason.value.trim() || undefined,
    })
    toasts.ok('Pinned', `${name.value.trim()} is now a GC root`)
    name.value = ''
    rev.value = ''
    reason.value = ''
    await pins.reload()
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}
async function remove(p: Pin) {
  try {
    await api.admin.unpin(props.repo, p.name)
    toasts.ok('Pin removed', p.name)
    await pins.reload()
  } catch (e) {
    toasts.error('Could not remove the pin', e)
  }
}
const columns: Column[] = [
  { key: 'name', label: 'Name', width: 'minmax(0, 1fr)' },
  { key: 'changeset', label: 'Changeset', width: 'minmax(0, 1fr)' },
  { key: 'reason', label: 'Reason', width: 'minmax(0, 1.4fr)', hideBelow: 800 },
  { key: 'created_at_ms', label: 'Pinned', width: '7rem' },
  { key: 'actions', label: 'Actions', width: '5rem', align: 'end', srOnlyHeader: true },
]
</script>

<template>
  <div class="pins">
    <Card
      title="Pins"
      icon="pin"
      subtitle="A pinned revision is a garbage-collection root: it and its history are never collected."
      flush
    >
      <ErrorState
        v-if="pins.error.value && !pins.data.value"
        what="pins"
        :error="pins.error.value"
        compact
        @retry="pins.reload()"
      />
      <DataTable
        v-else
        :columns="columns"
        :rows="pins.data.value ?? []"
        :row-key="(p: Pin) => p.name"
        caption="Pins"
        :loading="!pins.settled.value"
        dense
      >
        <template #cell-name="{ row }"
          ><strong class="mono">{{ (row as Pin).name }}</strong></template
        >
        <template #cell-changeset="{ row }"
          ><code class="mono">{{ shortId((row as Pin).changeset, 14) }}</code></template
        >
        <template #cell-reason="{ row }"
          ><span class="muted">{{ (row as Pin).reason || '–' }}</span></template
        >
        <template #cell-created_at_ms="{ row }"
          ><span class="faint"><RelativeTime :ms="(row as Pin).created_at_ms" /></span
        ></template>
        <template #cell-actions="{ row }"
          ><Button
            size="sm"
            variant="ghost"
            icon="trash"
            label="Remove pin"
            @click="remove(row as Pin)"
        /></template>
        <template #empty><EmptyState icon="pin" compact title="Nothing pinned" /></template>
      </DataTable>
    </Card>
    <Card title="Pin a revision" icon="plus">
      <form class="form" @submit.prevent="add">
        <TextField v-model="name" label="Name" mono required placeholder="release-1.0" />
        <TextField
          v-model="rev"
          label="Revision"
          mono
          required
          placeholder="main, a changeset id, prop_…"
        />
        <TextField v-model="reason" label="Reason" placeholder="Why keep it?" />
        <Alert v-if="error" tone="bad" live="alert" title="Could not pin">{{
          error.message
        }}</Alert>
        <div><Button type="submit" variant="primary" :loading="busy">Pin</Button></div>
      </form>
    </Card>
  </div>
</template>

<style scoped>
.pins {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.form {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-3);
  align-items: end;
}
.form :deep(.alert),
.form > div:last-child {
  grid-column: 1 / -1;
}
</style>
