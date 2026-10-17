<script setup lang="ts">
import { ref } from 'vue'
import { api } from '@/api/instance'
import type { AuditEntry } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { formatDateTime, shortId } from '@/lib/format'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const toasts = useToasts()

const contains = ref('')
const applied = ref('')
const limit = ref(100)
const audit = useResource<AuditEntry[]>(
  () => api.admin.audit(props.repo, { contains: applied.value || undefined, limit: limit.value }),
  { watch: [() => props.repo, applied, limit] },
)

const verify = ref<{ ok: boolean; entries: number; detail: string } | null>(null)
const verifying = ref(false)
async function runVerify() {
  verifying.value = true
  try {
    verify.value = await api.admin.verifyAudit(props.repo)
  } catch (e) {
    toasts.error('Could not verify the audit chain', e)
  } finally {
    verifying.value = false
  }
}

const columns: Column[] = [
  { key: 'seq', label: 'Seq', width: '4.5rem', align: 'end' },
  { key: 'timestamp_ms', label: 'When', width: '10rem', hideBelow: 800 },
  { key: 'actor', label: 'Actor', width: 'minmax(0, 1.2fr)' },
  { key: 'action', label: 'Action', width: 'minmax(0, 1fr)' },
  { key: 'target', label: 'Target', width: 'minmax(0, 1.2fr)', hideBelow: 900 },
  { key: 'hash', label: 'Chain hash', width: '8rem', hideBelow: 1100 },
]
const brief = (d: unknown) => {
  const s = JSON.stringify(d)
  return s && s.length > 140 ? `${s.slice(0, 140)}…` : (s ?? '')
}
</script>

<template>
  <div class="audit">
    <Card
      title="Hash chain"
      icon="shield"
      subtitle="Each entry commits to the one before it; tampering breaks every hash after it."
    >
      <div class="row">
        <Button icon="shield" :loading="verifying" data-testid="audit-verify" @click="runVerify"
          >Verify the chain</Button
        >
        <Alert
          v-if="verify"
          :tone="verify.ok ? 'ok' : 'bad'"
          :title="verify.ok ? 'Chain intact' : 'Chain broken'"
          live="status"
          class="grow"
          data-testid="audit-verify-result"
        >
          {{ verify.entries }} entries checked. {{ verify.detail }}
        </Alert>
      </div>
    </Card>
    <Card
      title="Audit log"
      icon="list"
      subtitle="Newest first. The same entries feed the live event stream."
      flush
    >
      <form class="tools" @submit.prevent="applied = contains.trim()">
        <TextField
          v-model="contains"
          label="Contains"
          mono
          placeholder="actor, target or detail text"
          class="f"
        />
        <Button type="submit" icon="search">Filter</Button>
      </form>
      <ErrorState
        v-if="audit.error.value && !audit.data.value"
        what="the audit log"
        :error="audit.error.value"
        compact
        @retry="audit.reload()"
      />
      <DataTable
        v-else
        :columns="columns"
        :rows="audit.data.value ?? []"
        :row-key="(e: AuditEntry) => String(e.seq)"
        caption="Audit log"
        :loading="!audit.settled.value"
        dense
      >
        <template #cell-timestamp_ms="{ row }"
          ><span class="faint">{{
            formatDateTime((row as AuditEntry).timestamp_ms)
          }}</span></template
        >
        <template #cell-actor="{ row }"
          ><IdentityLabel :identity="(row as AuditEntry).actor" :size="16"
        /></template>
        <template #cell-action="{ row }">
          <Badge size="sm" mono>{{ (row as AuditEntry).action }}</Badge>
          <p class="faint det">{{ brief((row as AuditEntry).details) }}</p>
        </template>
        <template #cell-target="{ row }"
          ><code class="mono">{{ shortId((row as AuditEntry).target, 20) }}</code></template
        >
        <template #cell-hash="{ row }"
          ><code class="mono faint">{{ shortId((row as AuditEntry).hash, 12) }}</code></template
        >
        <template #empty><EmptyState icon="list" compact title="No entries match" /></template>
      </DataTable>
      <div v-if="(audit.data.value?.length ?? 0) >= limit" class="more">
        <Button size="sm" @click="limit += 200">Show more</Button>
      </div>
    </Card>
  </div>
</template>

<style scoped>
.audit {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.row {
  display: flex;
  gap: var(--s-3);
  align-items: flex-start;
  flex-wrap: wrap;
}
.grow {
  flex: 1;
  min-width: 260px;
}
.tools {
  display: flex;
  gap: var(--s-2);
  align-items: flex-end;
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
}
.f {
  flex: 1;
  max-width: 480px;
}
.det {
  font-size: var(--fs-xs);
  font-family: var(--font-mono);
  overflow-wrap: anywhere;
}
.more {
  display: flex;
  justify-content: center;
  padding: var(--s-3);
}
</style>
