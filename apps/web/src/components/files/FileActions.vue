<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '@/api/instance'
import type { TreeEntry } from '@/api/types'
import { describeError } from '@/api/errors'
import { deleteFile, pathProblem, proposeChange } from '@/lib/edit'
import { moveFile } from '@/lib/fileOperations'
import { useUnsavedChanges } from '@/composables/useUnsavedChanges'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import TextField from '@/ui/TextField.vue'
import TextArea from '@/ui/TextArea.vue'
import Alert from '@/ui/Alert.vue'

const props = defineProps<{ repo: string; rev: string; stream: string; entry: TreeEntry }>()
const router = useRouter()
const board = useBoard()
const toasts = useToasts()
const mode = ref<'move' | 'delete' | null>(null)
const target = ref('')
const description = ref('')
const error = ref('')
const busy = ref(false)
useUnsavedChanges(
  computed(() => false),
  busy,
)
const problem = computed(() =>
  mode.value === 'move'
    ? pathProblem(target.value) ||
      (target.value.trim() === props.entry.path ? 'Choose a different file path.' : null)
    : null,
)
const requestTitle = computed(() =>
  mode.value === 'move'
    ? `Move ${props.entry.path} to ${target.value.trim()}`
    : `Delete ${props.entry.path}`,
)
function open(action: 'move' | 'delete') {
  mode.value = action
  target.value = props.entry.path
  description.value = ''
  error.value = ''
}
async function submit() {
  if (busy.value || problem.value || !mode.value) return
  busy.value = true
  error.value = ''
  try {
    if (mode.value === 'move') {
      const destination = await api.tree.resolve(props.repo, props.rev, target.value.trim())
      if (destination.kind !== 'missing')
        throw new Error(
          'This destination already exists. Choose another path; nothing has been overwritten.',
        )
    }
    const proposal = await proposeChange(api, {
      repo: props.repo,
      stream: props.stream,
      fromRev: props.rev,
      expectedBase: props.rev,
      title: requestTitle.value,
      description: description.value.trim() || undefined,
      changes:
        mode.value === 'move'
          ? moveFile(props.entry, target.value)
          : [deleteFile(props.entry.path)],
    })
    mode.value = null
    busy.value = false
    board.touched(['proposals', 'streams', 'predict'])
    toasts.ok('Change request opened', proposal.title)
    await router.push({
      name: 'proposal',
      params: { owner: props.repo.split('/')[0], name: props.repo.split('/')[1], id: proposal.id },
    })
  } catch (e) {
    error.value = describeError(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <span class="actions">
    <Button size="sm" @click="open('move')">Rename or move</Button>
    <Button size="sm" variant="danger" icon="trash" @click="open('delete')">Delete file</Button>
  </span>
  <Dialog
    :model-value="mode !== null"
    :title="mode === 'move' ? 'Rename or move file' : 'Delete file'"
    :dismissable="!busy"
    @update:model-value="mode = null"
  >
    <form id="file-operation" class="form" @submit.prevent="submit">
      <p class="source mono">{{ entry.path }}</p>
      <p>The branch will not change until this change request is reviewed and landed.</p>
      <TextField
        v-if="mode === 'move'"
        v-model="target"
        label="New path"
        :error="problem || undefined"
        :disabled="busy"
        hint="Use a new name or move it into another folder."
        autofocus
        required
        mono
      />
      <p v-else>Only this file will be removed. Its history stays available.</p>
      <TextArea v-model="description" label="Description (optional)" :disabled="busy" :rows="3" />
      <Alert v-if="error" title="Could not open the change request" tone="bad" live="alert">{{
        error
      }}</Alert>
    </form>
    <template #footer>
      <Button :disabled="busy" @click="mode = null">Cancel</Button>
      <Button
        type="submit"
        form="file-operation"
        variant="primary"
        :loading="busy"
        :disabled="!!problem"
        >Open change request</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.actions {
  display: inline-flex;
  gap: var(--s-2);
  flex-wrap: wrap;
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.source {
  overflow-wrap: anywhere;
}
</style>
