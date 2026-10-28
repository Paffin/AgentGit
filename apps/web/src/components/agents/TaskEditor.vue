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
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'

const props = defineProps<{ repo: string; task: Task; hasRuns: boolean }>()
const emit = defineEmits<{ saved: [task: Task]; reload: [] }>()
const auth = useAuth()
const allowed = computed(
  () =>
    auth.capabilities?.features.includes('tasks.edit') &&
    auth.can(props.repo, 'createTask') &&
    props.task.state === 'open' &&
    !props.hasRuns &&
    (samePrincipal(props.task.created_by, auth.identity) ||
      auth.can(props.repo, 'controlExecution')),
)
const open = ref(false),
  busy = ref(false),
  error = ref(''),
  stale = ref(false)
const title = ref(''),
  description = ref(''),
  paths = ref(''),
  saved = ref(false)
type Content = Pick<Task, 'title' | 'description' | 'scope'>
let expected: Content
let generation = 0,
  key = randomHex(),
  controller: AbortController | undefined
const content = computed<Content>(() => ({
  title: title.value,
  description: description.value,
  scope: paths.value
    .split('\n')
    .map((value) => value.trim())
    .filter(Boolean),
}))
const changed = computed(() => JSON.stringify(content.value) !== JSON.stringify(expected))
watch(
  content,
  () => {
    key = randomHex()
  },
  { flush: 'sync' },
)
function reset() {
  generation++
  controller?.abort()
  open.value = false
  busy.value = false
  saved.value = false
  title.value = description.value = paths.value = error.value = ''
  stale.value = false
}
watch([() => props.repo, () => props.task.id, () => auth.token], reset, { flush: 'sync' })
onBeforeUnmount(reset)
function begin() {
  if (!allowed.value) return
  expected = {
    title: props.task.title,
    description: props.task.description,
    scope: [...props.task.scope],
  }
  title.value = expected.title
  description.value = expected.description
  paths.value = expected.scope.join('\n')
  key = randomHex()
  error.value = ''
  stale.value = false
  saved.value = false
  open.value = true
}
async function save() {
  if (busy.value || stale.value || !allowed.value || !title.value.trim() || !changed.value) return
  const mine = generation
  const repository = props.repo,
    id = props.task.id
  const submitted = content.value
  const options = {
    token: auth.token,
    signal: (controller = new AbortController()).signal,
    idempotencyKey: key,
    quietUnauthenticated: true,
  }
  busy.value = true
  error.value = ''
  try {
    const task = await api.agents.updateTask(repository, id, submitted, expected, options)
    if (mine !== generation) return
    open.value = false
    saved.value = true
    emit('saved', task)
  } catch (cause) {
    if (mine !== generation) return
    error.value = describeError(cause)
    stale.value = isApiError(cause) && cause.status === 412
    emit('reload')
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <Button v-if="allowed" @click="begin">Edit task…</Button>
  <span v-if="saved" role="status">Task instructions saved.</span>
  <Dialog v-model="open" title="Edit task instructions" :dismissable="!busy" size="lg">
    <Alert v-if="error" tone="bad" live="alert" title="Task was not saved">
      {{ error }}
      <p v-if="stale">
        Your draft is kept here. Copy any text you need, then close this dialog and reopen it to
        review the current task. If work has started, create a follow-up task.
      </p>
    </Alert>
    <p class="hint">
      Update the instructions before the first run. Assignment, prerequisites and source links stay
      unchanged.
    </p>
    <form id="edit-task-instructions" class="fields" @submit.prevent="save">
      <TextField v-model="title" label="Title" required autofocus :disabled="busy" />
      <TextArea
        v-model="description"
        label="Description"
        :rows="7"
        hint="Describe the expected result and how to verify it."
        :disabled="busy"
      />
      <TextArea
        v-model="paths"
        label="Scope (path globs)"
        :rows="3"
        mono
        hint="One path pattern per line. An empty scope does not restrict paths; repository policy still applies."
        :disabled="busy"
      />
    </form>

    <template #footer>
      <Button :disabled="busy" @click="open = false">Cancel</Button>
      <Button
        type="submit"
        form="edit-task-instructions"
        variant="primary"
        :loading="busy"
        :disabled="!changed || !title.trim() || stale || !allowed"
        >Save instructions</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.fields {
  display: grid;
  gap: 1rem;
}
.hint {
  color: var(--text-2);
  margin: 0 0 1rem;
}
</style>
