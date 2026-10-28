<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useResource } from '@/composables/useResource'
import { useLiveReload } from '@/composables/useLiveReload'
import Card from '@/ui/Card.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import Alert from '@/ui/Alert.vue'
import TextArea from '@/ui/TextArea.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{ repo: string }>()
const auth = useAuth()
const supported = computed(() => !!auth.capabilities?.features.includes('task.scheduling'))
const canControl = computed(() => auth.can(props.repo, 'controlExecution'))
const state = useResource(
  (signal) =>
    supported.value
      ? api.agents.scheduling(props.repo, { signal, token: auth.token, quietUnauthenticated: true })
      : Promise.resolve(null),
  { watch: [() => props.repo, () => auth.token, supported] },
)
useLiveReload(['task'], () => state.reload())
const dialog = ref(false),
  targetPaused = ref(false),
  reason = ref(''),
  busy = ref(false),
  error = ref('')
let version = 0,
  context = 0,
  controller: AbortController | undefined
watch(
  [() => props.repo, () => auth.token],
  () => {
    context++
    controller?.abort()
    dialog.value = false
    busy.value = false
    error.value = ''
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
})
function begin() {
  if (!state.data.value || !canControl.value) return
  version = state.data.value.version
  targetPaused.value = !state.data.value.paused
  reason.value = ''
  error.value = ''
  dialog.value = true
}
async function apply() {
  if (busy.value || !canControl.value) return
  const mine = context
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.agents.setScheduling(
      props.repo,
      { paused: targetPaused.value, reason: reason.value, expected_version: version },
      { token: auth.token, signal: controller.signal, quietUnauthenticated: true },
    )
    if (mine !== context) return
    dialog.value = false
    await state.reload()
  } catch (e) {
    if (mine === context) error.value = describeError(e)
  } finally {
    if (mine === context) busy.value = false
  }
}
</script>

<template>
  <Card v-if="supported" title="Task execution" icon="play">
    <Alert v-if="state.error.value" tone="bad"
      >{{ describeError(state.error.value) }}
      <Button size="sm" @click="state.reload()">Reload execution state</Button></Alert
    >
    <p v-else-if="!state.data.value" role="status">Loading execution state…</p>
    <template v-else>
      <div class="execution-state">
        <div>
          <strong>{{
            state.data.value.paused ? 'New task runs are paused' : 'New task runs are allowed'
          }}</strong>
          <p v-if="state.data.value.reason">{{ state.data.value.reason }}</p>
          <p class="muted">
            Configured executors can pick up ready tasks when allowed. Existing runs keep working;
            cancel their tasks separately to stop them.
          </p>
          <p v-if="state.data.value.updated_by" class="muted">
            Changed by {{ state.data.value.updated_by }} ·
            <RelativeTime :ms="state.data.value.updated_at_ms" />
          </p>
        </div>
        <Button v-if="canControl" :disabled="busy" @click="begin">{{
          state.data.value.paused ? 'Resume task execution…' : 'Pause new task runs…'
        }}</Button>
      </div>
      <p v-if="!canControl" class="muted">
        A repository operator or administrator can change this setting.
      </p>
    </template>
    <Dialog
      v-model="dialog"
      :title="targetPaused ? 'Pause new task runs?' : 'Resume task execution?'"
      :busy="busy"
    >
      <p v-if="targetPaused">
        The server will refuse new task-bound runs. Already running work continues. You can still
        create tasks and review changes.
      </p>
      <p v-else>
        All ready tasks become eligible for configured executors, including tasks queued during the
        pause. This does not start an unconfigured executor.
      </p>
      <TextArea
        v-if="targetPaused"
        v-model="reason"
        label="Reason (optional)"
        :rows="3"
        :disabled="busy"
        hint="Visible to everyone who can read this repository."
      />
      <Alert v-if="error" tone="bad"
        >{{ error }} Close this dialog and review the current state before retrying.</Alert
      >
      <template #footer
        ><Button :disabled="busy" @click="dialog = false">Keep current setting</Button
        ><Button variant="primary" :loading="busy" @click="apply">{{
          targetPaused ? 'Pause new task runs' : 'Resume task execution'
        }}</Button></template
      >
    </Dialog>
  </Card>
</template>

<style scoped>
.execution-state {
  display: flex;
  justify-content: space-between;
  align-items: start;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.execution-state > div {
  flex: 1;
  min-width: 220px;
}
.muted {
  color: var(--text-2);
}
</style>
