<script setup lang="ts">
import { computed, ref } from 'vue'
import { api } from '@/api/instance'
import type { Lease } from '@/api/types'
import { useLiveReload } from '@/composables/useLiveReload'
import { useResource } from '@/composables/useResource'
import { useServerNow } from '@/composables/useNow'
import { formatCountdown, formatDateTime } from '@/lib/format'
import { requirementText } from '@/lib/roles'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import Segmented from '@/ui/Segmented.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const auth = useAuth()
const board = useBoard()
const toasts = useToasts()
const now = useServerNow()

const scope = ref<'active' | 'all'>('active')
const leases = useResource<Lease[]>(() => api.agents.leases(props.repo, scope.value === 'active'), {
  watch: [() => props.repo, scope],
})
useLiveReload(['lease', 'run'], () => leases.reload())

const columns: Column[] = [
  { key: 'scope', label: 'Scope', width: 'minmax(0, 1.4fr)' },
  { key: 'kind', label: 'Kind', width: '9.5rem' },
  { key: 'holder', label: 'Holder', width: 'minmax(0, 1.4fr)' },
  { key: 'expires_at_ms', label: 'Expires', width: '10rem' },
  { key: 'actions', label: 'Actions', width: '9rem', align: 'end', srOnlyHeader: true },
]
const rows = computed(() =>
  [...(leases.data.value ?? [])].sort((a, b) => b.acquired_at_ms - a.acquired_at_ms),
)
const canManage = computed(() => auth.can(props.repo, 'manageLeases'))
const state = (l: Lease) =>
  l.released_at_ms ? 'released' : l.expires_at_ms <= now.value ? 'expired' : 'active'
const options = [
  { value: 'active', label: 'Active' },
  { value: 'all', label: 'History' },
] as const

async function release(l: Lease) {
  try {
    await api.agents.releaseLease(props.repo, l.id)
    toasts.ok('Lease released', l.scope)
    await leases.reload()
    board.touched(['leases', 'predict'])
  } catch (e) {
    toasts.error('Could not release the lease', e)
  }
}
/** One click extends by this much; the server refuses what its lease limits do not allow and says why. */
const EXTEND_MS = 15 * 60_000
async function extend(l: Lease) {
  try {
    await api.agents.extendLease(props.repo, l.id, EXTEND_MS)
    toasts.ok(`Lease extended by ${EXTEND_MS / 60_000} minutes`, l.scope)
    await leases.reload()
  } catch (e) {
    toasts.error('Could not extend the lease', e)
  }
}
</script>

<template>
  <div class="leases">
    <Segmented v-model="scope" legend="Show" :options="options" />
    <ErrorState
      v-if="leases.error.value && !leases.data.value"
      what="leases"
      :error="leases.error.value"
      @retry="leases.reload()"
    />
    <DataTable
      v-else
      :columns="columns"
      :rows="rows"
      :row-key="(l: Lease) => l.id"
      caption="Leases"
      :loading="!leases.settled.value"
    >
      <template #cell-scope="{ row }"
        ><code class="mono">{{ (row as Lease).scope }}</code></template
      >
      <template #cell-kind="{ row }">
        <Badge v-if="(row as Lease).exclusive" tone="warn" icon="lock" size="sm"
          >exclusive #{{ (row as Lease).fencing_token }}</Badge
        >
        <Badge v-else icon="unlock" size="sm">soft</Badge>
      </template>
      <template #cell-holder="{ row }"
        ><IdentityLabel :identity="(row as Lease).holder" :size="18"
      /></template>
      <template #cell-expires_at_ms="{ row }">
        <span v-if="state(row as Lease) === 'active'">{{
          formatCountdown((row as Lease).expires_at_ms, now)
        }}</span>
        <span v-else class="faint"
          >{{ state(row as Lease) }} ·
          {{ formatDateTime((row as Lease).released_at_ms || (row as Lease).expires_at_ms) }}</span
        >
      </template>
      <template #cell-actions="{ row }">
        <span v-if="state(row as Lease) === 'active'" class="acts">
          <Button
            size="sm"
            variant="ghost"
            :disabled="!canManage"
            :disabled-reason="requirementText('manageLeases')"
            @click="extend(row as Lease)"
            >Extend</Button
          >
          <Button
            size="sm"
            variant="ghost"
            :disabled="!canManage"
            :disabled-reason="requirementText('manageLeases')"
            @click="release(row as Lease)"
            >Release</Button
          >
        </span>
      </template>
      <template #empty
        ><EmptyState
          icon="unlock"
          compact
          title="No leases"
          description="Agents claim path scopes with soft or exclusive leases."
      /></template>
    </DataTable>
  </div>
</template>

<style scoped>
.leases {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.acts {
  display: inline-flex;
  gap: 2px;
}
</style>
