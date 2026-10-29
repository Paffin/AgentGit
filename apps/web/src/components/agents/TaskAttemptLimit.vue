<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { randomHex } from '@/api/client'
import { describeError, isApiError } from '@/api/errors'
import type { Task } from '@/api/types'
import { samePrincipal } from '@/lib/identity'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import TextField from '@/ui/TextField.vue'

const props = defineProps<{ repo: string; task: Task }>()
const emit = defineEmits<{ saved: [task: Task]; reload: [] }>()
const auth = useAuth()
const supported = computed(() => !!auth.capabilities?.features.includes('tasks.attempt_limits'))
const allowed = computed(
  () =>
    supported.value &&
    auth.can(props.repo, 'createTask') &&
    ['open', 'in_progress'].includes(props.task.state) &&
    (auth.can(props.repo, 'controlExecution') ||
      (props.task.attempts_started === 0 && samePrincipal(props.task.created_by, auth.identity))),
)
const open = ref(false),
  busy = ref(false),
  error = ref(''),
  blocked = ref(false)
const limit = ref('')
const expected = ref(0)
const valid = computed(() => /^\d+$/.test(limit.value) && Number(limit.value) <= 10_000)
const changed = computed(() => valid.value && Number(limit.value) !== expected.value)
let generation = 0,
  key = randomHex(),
  controller: AbortController | undefined
watch(
  limit,
  () => {
    key = randomHex()
  },
  { flush: 'sync' },
)
function reset() {
  generation++
  controller?.abort()
  open.value = busy.value = blocked.value = false
  error.value = limit.value = ''
}
watch([() => props.repo, () => props.task.id, () => auth.token, () => auth.identity], reset, {
  flush: 'sync',
})
onBeforeUnmount(reset)
function begin() {
  if (!allowed.value) return
  expected.value = props.task.max_attempts
  limit.value = String(expected.value)
  error.value = ''
  blocked.value = false
  key = randomHex()
  open.value = true
}
async function save() {
  if (busy.value || blocked.value || !allowed.value || !changed.value) return
  const mine = generation
  busy.value = true
  error.value = ''
  try {
    const updated = await api.agents.setTaskAttemptLimit(
      props.repo,
      props.task.id,
      Number(limit.value),
      expected.value,
      {
        token: auth.token,
        signal: (controller = new AbortController()).signal,
        idempotencyKey: key,
        quietUnauthenticated: true,
      },
    )
    if (mine !== generation) return
    open.value = false
    emit('saved', updated)
  } catch (cause) {
    if (mine !== generation) return
    error.value = describeError(cause)
    blocked.value = isApiError(cause) && [401, 403, 412].includes(cause.status)
    emit('reload')
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <section v-if="supported" aria-label="Task attempts">
    <h2>Attempts</h2>
    <p>
      {{ task.attempts_started }} started ·
      {{ task.max_attempts ? `Limit: ${task.max_attempts}` : 'No task-local limit' }}
    </p>
    <Button v-if="allowed" @click="begin">Change attempt limit…</Button>
    <Dialog v-model="open" title="Change attempt limit" :dismissable="!busy">
      <p>
        Attempts started: {{ task.attempts_started }}. Changing this limit affects future attempts;
        it does not stop a running attempt or resume a finished task.
      </p>
      <Alert v-if="error" tone="bad" live="alert" title="Attempt limit was not saved">
        {{ error }}
        <p v-if="blocked">
          Your draft is kept here. Close and reopen this dialog to review the current limit before
          trying again. If your session expired, sign in again.
        </p>
      </Alert>
      <form id="task-attempt-limit" @submit.prevent="save">
        <TextField
          v-model="limit"
          label="Maximum attempts"
          type="number"
          :min="0"
          :max="10000"
          required
          autofocus
          :disabled="busy"
          hint="Use 0 for no task-local limit. Repository execution controls still apply."
        />
      </form>
      <template #footer>
        <Button :disabled="busy" @click="open = false">Keep current limit</Button>
        <Button
          type="submit"
          form="task-attempt-limit"
          variant="primary"
          :loading="busy"
          :disabled="!changed || blocked || !allowed"
          >Confirm attempt limit</Button
        >
      </template>
    </Dialog>
  </section>
</template>
