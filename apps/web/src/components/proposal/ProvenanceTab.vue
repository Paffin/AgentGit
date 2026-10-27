<script setup lang="ts">
import { computed } from 'vue'
import type { ProvenanceEntry, ProvenanceRecord } from '@/api/types'
import { shortId } from '@/lib/format'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import EmptyState from '@/ui/EmptyState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{ entries: readonly ProvenanceEntry[]; authorIsAgent: boolean }>()

type Shown = Omit<ProvenanceEntry, 'record'> & { record: ProvenanceRecord }
const shown = computed<Shown[]>(() =>
  props.entries.flatMap((e) => (e.record ? [{ ...e, record: e.record }] : [])),
)
</script>

<template>
  <div class="prov" data-testid="provenance-tab">
    <Alert tone="info" title="An audit trail, not proof">
      An agent signs this record with its own key, which proves who wrote it and that it was not
      altered. It does not prove the claims are true: the model, rationale and inputs below are
      <strong>untrusted text</strong> written by the agent. Correctness comes from validation and
      review.
    </Alert>
    <EmptyState
      v-if="shown.length === 0"
      icon="sparkle"
      compact
      title="Nothing recorded about where this came from"
      :description="
        authorIsAgent
          ? 'This change was authored by an agent, but nobody attached a signed provenance record. Policy may require one before it can land.'
          : 'Provenance is attached by agents; this change was not authored by one.'
      "
    />
    <ul v-else class="list" role="list">
      <li v-for="e in shown" :key="e.id" class="entry" :data-provenance="e.id">
        <div class="top">
          <IdentityLabel :identity="e.record.agent" />
          <Badge v-if="e.verified" tone="ok" icon="shield" size="sm">signature verified</Badge>
          <Badge v-else tone="bad" icon="alert" size="sm">signature not verified</Badge>
          <span class="faint when">recorded <RelativeTime :ms="e.created_at_ms" /></span>
        </div>
        <dl class="kv">
          <div v-if="e.record.run_id">
            <dt>Run</dt>
            <dd class="mono">{{ e.record.run_id }}</dd>
          </div>
          <div v-if="e.record.model">
            <dt>Model</dt>
            <dd class="mono">{{ e.record.model }}</dd>
          </div>
          <div v-if="e.record.task_id">
            <dt>Task</dt>
            <dd class="mono">{{ shortId(e.record.task_id, 14) }}</dd>
          </div>
          <div v-if="e.record.prompt_digest">
            <dt>Prompt digest</dt>
            <dd class="mono">{{ e.record.prompt_digest }}</dd>
          </div>
          <div>
            <dt>Changeset</dt>
            <dd class="mono">{{ shortId(e.record.changeset, 16) }}</dd>
          </div>
          <div>
            <dt>Signing key</dt>
            <dd class="mono">{{ shortId(e.public_key, 16) }}</dd>
          </div>
        </dl>
        <div v-if="Object.keys(e.record.parameters).length" class="block">
          <h4>Parameters</h4>
          <p class="chips">
            <Badge v-for="(v, k) in e.record.parameters" :key="k" mono>{{ k }} = {{ v }}</Badge>
          </p>
        </div>
        <div v-if="e.record.checks_run.length" class="block">
          <h4>Checks the agent ran itself <span class="faint">(informational)</span></h4>
          <ul role="list" class="plain">
            <li v-for="c in e.record.checks_run" :key="c" class="mono">{{ c }}</li>
          </ul>
        </div>
        <div v-if="e.record.inputs.length" class="block">
          <h4>Inputs consulted <span class="faint">(untrusted text)</span></h4>
          <ul role="list" class="plain">
            <li v-for="(i, n) in e.record.inputs" :key="n" class="text">{{ i }}</li>
          </ul>
        </div>
        <div v-if="e.record.rationale" class="block">
          <h4>Rationale <span class="faint">(untrusted text, written by the agent)</span></h4>
          <blockquote class="rationale">{{ e.record.rationale }}</blockquote>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.prov {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.entry {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.top {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.when {
  margin-left: auto;
  font-size: var(--fs-sm);
}
.kv {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: var(--s-2) var(--s-4);
  margin: 0;
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
h4 {
  font-size: var(--fs-sm);
  margin-bottom: 4px;
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}
.plain {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: var(--fs-sm);
}
.text {
  overflow-wrap: anywhere;
  white-space: pre-wrap;
}
.rationale {
  margin: 0;
  padding: var(--s-3);
  border-left: 3px solid var(--agent-line);
  background: var(--agent-soft);
  border-radius: 0 var(--r-sm) var(--r-sm) 0;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
