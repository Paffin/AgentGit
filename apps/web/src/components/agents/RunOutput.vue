<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { useAuth } from '@/stores/auth'
import { useResource } from '@/composables/useResource'
import { useLiveReload } from '@/composables/useLiveReload'
import { describeError } from '@/api/errors'
import { formatBytes } from '@/lib/format'
import { downloadTextFile } from '@/lib/download'
import Button from '@/ui/Button.vue'
import Tabs from '@/ui/Tabs.vue'
import Alert from '@/ui/Alert.vue'
import ErrorState from '@/ui/ErrorState.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Dialog from '@/ui/Dialog.vue'

const props = defineProps<{ repo: string; runId: string }>()
const auth = useAuth()
const supported = computed(() => !!auth.capabilities?.features.includes('agents.run_output'))
const output = useResource(
  (signal) => api.agents.runOutput(props.repo, props.runId, { signal, token: auth.token }),
  {
    watch: [() => props.repo, () => props.runId, () => auth.token, supported],
    enabled: () => supported.value,
  },
)
useLiveReload(['run'], () => output.reload())
const tab = ref<'stdout' | 'stderr'>('stdout'),
  confirming = ref(false),
  busy = ref(false),
  error = ref('')
const text = computed(() => output.data.value?.[tab.value] || '')
const total = computed(
  () => output.data.value?.[tab.value === 'stdout' ? 'stdout_bytes' : 'stderr_bytes'] || 0,
)
const canDelete = computed(() => auth.can(props.repo, 'controlExecution'))
let context = 0
onBeforeUnmount(() => {
  context++
})
watch(
  [() => props.repo, () => props.runId, () => auth.token],
  () => {
    context++
    confirming.value = false
    busy.value = false
    error.value = ''
  },
  { flush: 'sync' },
)
async function remove() {
  const mine = context,
    token = auth.token
  busy.value = true
  error.value = ''
  try {
    await api.agents.deleteRunOutput(props.repo, props.runId, { token })
    if (mine !== context) return
    confirming.value = false
    await output.reload()
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  } finally {
    if (mine === context) busy.value = false
  }
}
</script>

<template>
  <section v-if="supported" aria-labelledby="output-heading" class="output">
    <div class="bar">
      <h2 id="output-heading">Saved executor output</h2>
      <Button size="sm" @click="output.reload()">Refresh output</Button>
    </div>
    <ErrorState v-if="output.error.value" :error="output.error.value" @retry="output.reload()" />
    <p v-else-if="!output.settled.value" role="status">Loading output…</p>
    <p v-else-if="!output.data.value?.available" class="muted">
      No output saved. An operator must enable output capture for the executor; an opted-in executor
      saves a bounded tail when the session ends.
    </p>
    <template v-else>
      <p class="muted">
        Executor-provided text, saved <RelativeTime :ms="output.data.value.captured_at_ms" />. At
        most 32 KiB per stream are retained. This is diagnostic output, not validation evidence.
      </p>
      <Tabs
        v-model="tab"
        :tabs="[
          { id: 'stdout', label: 'Standard output' },
          { id: 'stderr', label: 'Standard error' },
        ]"
        label="Output stream"
      >
        <template #default>
          <p class="muted">
            {{ formatBytes(total) }} emitted in this stream; only the captured tail may be shown.
          </p>
          <pre
            tabindex="0"
            :aria-label="tab === 'stdout' ? 'Captured standard output' : 'Captured standard error'"
            >{{ text || '(empty)' }}</pre>
        </template>
      </Tabs>
      <div class="bar">
        <Button size="sm" @click="downloadTextFile(`${runId}-${tab}.txt`, text)"
          >Download this stream</Button
        >
        <Button v-if="canDelete" size="sm" variant="danger" @click="confirming = true"
          >Delete saved output…</Button
        >
      </div>
    </template>
    <Dialog v-model="confirming" title="Delete saved executor output?" :dismissable="!busy">
      <p>
        The captured text will be removed. The run and its outcome stay in the audit history.
        Further output uploads for this run will be refused.
      </p>
      <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
      <template #footer
        ><Button :disabled="busy" @click="confirming = false">Keep output</Button
        ><Button variant="danger" :loading="busy" @click="remove">Delete output</Button></template
      >
    </Dialog>
  </section>
</template>

<style scoped>
.output {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  min-width: 0;
}
.bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--s-2);
}
pre {
  max-height: 28rem;
  overflow: auto;
  padding: var(--s-3);
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  white-space: pre;
  font-size: var(--fs-sm);
}
pre:focus-visible {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}
</style>
