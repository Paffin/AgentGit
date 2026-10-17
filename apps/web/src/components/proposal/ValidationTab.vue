<script setup lang="ts">
import { computed } from 'vue'
import type { Attestation, Preparation } from '@/api/types'
import { shortId } from '@/lib/format'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import CopyButton from '@/ui/CopyButton.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{
  prep?: Preparation | null
  attestations: readonly Attestation[]
  canRequest: boolean
  requestReason: string
  requesting: boolean
  open: boolean
}>()
defineEmits<{ request: [] }>()

const TONE = { passed: 'ok', failed: 'bad', pending: 'info', missing: 'warn' } as const
const tone = (s: string) => (s in TONE ? TONE[s as keyof typeof TONE] : 'neutral')
const checkColumns: Column[] = [
  { key: 'check', label: 'Required check', width: 'minmax(0, 1fr)' },
  { key: 'state', label: 'State', width: '8rem' },
  { key: 'evidence', label: 'Evidence', width: 'minmax(0, 1.4fr)' },
]
const jobById = computed(() => new Map((props.prep?.jobs ?? []).map((j) => [j.id, j])))
const rows = computed(() => props.prep?.validations ?? [])
const tree = computed(() => props.prep?.candidate_tree ?? '')
</script>

<template>
  <div class="validation" data-testid="validation-tab">
    <div class="bar">
      <p class="muted">
        A check counts only if a trusted issuer signed a passing result for
        <em>this exact tree</em>
        <code v-if="tree" class="mono">{{ shortId(tree, 10) }}</code> under the current policy.
      </p>
      <Button
        icon="play"
        :loading="requesting"
        :disabled="!canRequest || !open || rows.length === 0"
        :disabled-reason="
          !open
            ? 'Only open proposals are validated'
            : rows.length === 0
              ? 'The policy requires no checks'
              : requestReason
        "
        @click="$emit('request')"
      >
        Request validation
      </Button>
    </div>

    <DataTable
      v-if="rows.length"
      :columns="checkColumns"
      :rows="rows"
      :row-key="(r) => r.check"
      caption="Required checks"
      dense
    >
      <template #cell-check="{ row }"
        ><code class="mono">{{ row.check }}</code></template
      >
      <template #cell-state="{ row }"
        ><Badge :tone="tone(row.state)" size="sm">{{ row.state }}</Badge></template
      >
      <template #cell-evidence="{ row }">
        <span v-if="row.attestation_id" class="mono ev"
          >attestation {{ shortId(row.attestation_id, 12) }}</span
        >
        <span v-else-if="row.job_id" class="ev">
          <span class="mono">job {{ shortId(row.job_id, 12) }}</span>
          <Badge size="sm" :tone="jobById.get(row.job_id)?.state === 'failed' ? 'bad' : 'info'">{{
            jobById.get(row.job_id)?.state ?? 'queued'
          }}</Badge>
        </span>
        <span v-else-if="row.state === 'missing'" class="faint"
          >nothing yet: no attestation, no job</span
        >
        <span v-else class="faint">–</span>
      </template>
    </DataTable>
    <EmptyState
      v-else
      icon="shield"
      compact
      title="No checks required"
      description="The stream’s policy lists no required validations, so nothing needs an attestation."
    />

    <section v-if="(prep?.jobs ?? []).length" aria-labelledby="jobs-h" class="sec">
      <h3 id="jobs-h">Jobs for this tree</h3>
      <ul class="jobs" role="list">
        <li v-for="j in prep?.jobs" :key="j.id">
          <Badge
            size="sm"
            :tone="j.state === 'succeeded' ? 'ok' : j.state === 'failed' ? 'bad' : 'info'"
            >{{ j.state }}</Badge
          >
          <code class="mono">{{ j.validation?.check }}</code>
          <span class="faint"
            >runs <code class="mono">{{ (j.validation?.run?.argv ?? []).join(' ') }}</code></span
          >
          <span v-if="j.claimed_by" class="faint">claimed by {{ j.claimed_by }}</span>
          <span v-if="j.error" class="err">{{ j.error }}</span>
        </li>
      </ul>
    </section>

    <section aria-labelledby="att-h" class="sec">
      <h3 id="att-h">Attestations for this tree</h3>
      <EmptyState
        v-if="attestations.length === 0"
        icon="shield"
        compact
        title="No attestations yet"
        :description="
          rows.length
            ? 'A runner with the runner role signs one when a check finishes.'
            : 'Nothing was signed for this tree.'
        "
      />
      <ul v-else class="atts" role="list">
        <li v-for="a in attestations" :key="a.id" class="att" :data-attestation="a.id">
          <div class="r1">
            <Badge
              :tone="a.result === 'pass' ? 'ok' : 'bad'"
              :icon="a.result === 'pass' ? 'check' : 'x'"
              >{{ a.result }}</Badge
            >
            <code class="mono">{{ a.check }}</code>
            <span class="faint">issued by</span>
            <IdentityLabel :identity="a.issuer" :size="16" />
            <span class="faint when"><RelativeTime :ms="a.issued_at_ms" /></span>
          </div>
          <dl class="meta">
            <div>
              <dt>Tree</dt>
              <dd class="mono">
                {{ shortId(a.tree_id, 16) }}
                <CopyButton :text="a.tree_id" label="Copy tree id" />
              </dd>
            </div>
            <div>
              <dt>Policy version</dt>
              <dd>{{ a.policy_version }}</dd>
            </div>
            <div>
              <dt>Environment digest</dt>
              <dd class="mono">
                <template v-if="a.environment_digest">{{ a.environment_digest }}</template>
                <span v-else class="faint">not pinned</span>
              </dd>
            </div>
            <div>
              <dt>Signing key</dt>
              <dd class="mono">{{ shortId(a.public_key, 16) }}</dd>
            </div>
            <div>
              <dt>Id</dt>
              <dd class="mono">{{ a.id }}</dd>
            </div>
          </dl>
        </li>
      </ul>
      <Alert tone="info">
        The server verified each signature when it was submitted. Whether an attestation
        <em>counts</em> is decided by the policy’s trusted issuers, which is what the table above
        reflects.
      </Alert>
    </section>
  </div>
</template>

<style scoped>
.validation {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.sec {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
h3 {
  font-size: var(--fs-md);
}
.ev {
  display: inline-flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}
.jobs,
.atts {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.jobs li {
  display: flex;
  gap: var(--s-3);
  align-items: center;
  flex-wrap: wrap;
  font-size: var(--fs-sm);
}
.err {
  color: var(--bad);
}
.att {
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  background: var(--surface);
  padding: var(--s-3) var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.r1 {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}
.when {
  margin-left: auto;
}
.meta {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: var(--s-2) var(--s-4);
  margin: 0;
}
.meta div {
  min-width: 0;
}
dt {
  font-size: var(--fs-xs);
  color: var(--text-3);
}
dd {
  margin: 0;
  overflow-wrap: anywhere;
  font-size: var(--fs-sm);
}
</style>
