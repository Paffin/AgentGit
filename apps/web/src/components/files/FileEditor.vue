<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import { pathProblem, proposeChange, putFile } from '@/lib/edit'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import { useUnsavedChanges } from '@/composables/useUnsavedChanges'
import { useResource } from '@/composables/useResource'
import ErrorState from '@/ui/ErrorState.vue'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'

/**
 * Edit a text file (or write a new one) and propose the change, all in the browser. The edit is applied to a draft
 * on the server and becomes a change request; nothing lands until it is reviewed and landed like any other.
 */
const props = defineProps<{
  repo: string
  /** The file being edited; for a new file this is the directory it will go in (may be empty). */
  path: string
  /** What the file contains now (empty for a new file). */
  original: string
  /** The revision on screen, so the edit starts from exactly what was read. */
  rev: string
  /** The branch the change request is for. */
  stream: string
  executable?: boolean
  fileType?: string
  isNew?: boolean
}>()
const emit = defineEmits<{ cancel: [] }>()

const router = useRouter()
const board = useBoard()
const toasts = useToasts()

const initialPath = props.isNew ? (props.path ? `${props.path}/` : '') : props.path
const newPath = ref(initialPath)
const text = ref(props.original)
const filePath = computed(() => (props.isNew ? newPath.value.trim() : props.path))
const problem = computed(() => (props.isNew && newPath.value ? pathProblem(newPath.value) : null))
const changed = computed(() =>
  props.isNew ? filePath.value !== '' : text.value !== props.original,
)
const base = useResource(async () =>
  props.isNew ? (await api.streams.head(props.repo, props.stream)).head : props.rev,
)
const canPropose = computed(
  () =>
    changed.value &&
    base.settled.value &&
    !base.error.value &&
    !(props.isNew && pathProblem(newPath.value) !== null),
)

const asking = ref(false)
const title = ref('')
const description = ref('')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const submitted = ref(false)
const dirty = computed(
  () => !submitted.value && (text.value !== props.original || newPath.value !== initialPath),
)
const { askingToLeave, requestLeave, decide } = useUnsavedChanges(dirty, busy)
watch(askingToLeave, (open) => {
  if (open) asking.value = false
})

async function cancel() {
  if (await requestLeave()) emit('cancel')
}

let draftUrl = ''
function downloadDraft() {
  if (draftUrl) URL.revokeObjectURL(draftUrl)
  draftUrl = URL.createObjectURL(new Blob([text.value], { type: 'text/plain;charset=utf-8' }))
  const link = document.createElement('a')
  link.href = draftUrl
  link.download = filePath.value.split('/').pop() || 'draft.txt'
  document.body.appendChild(link)
  link.click()
  link.remove()
}
onBeforeUnmount(() => {
  if (draftUrl) URL.revokeObjectURL(draftUrl)
})

function ask() {
  title.value = props.isNew ? `Add ${filePath.value}` : `Update ${filePath.value}`
  description.value = ''
  error.value = null
  asking.value = true
}

async function propose() {
  busy.value = true
  error.value = null
  try {
    const p = await proposeChange(api, {
      repo: props.repo,
      stream: props.stream,
      fromRev: base.data.value || undefined,
      expectedBase: base.data.value,
      changes: [
        putFile(
          filePath.value,
          text.value,
          props.executable === true,
          props.fileType === 'symlink' ? 'symlink' : undefined,
        ),
      ],
      title: title.value.trim(),
      description: description.value.trim() || undefined,
    })
    submitted.value = true
    busy.value = false
    asking.value = false
    board.touched(['proposals', 'streams', 'predict'])
    toasts.ok('Change request opened', p.title)
    await router.push({
      name: 'proposal',
      params: { owner: props.repo.split('/')[0], name: props.repo.split('/')[1], id: p.id },
    })
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="editor" data-testid="file-editor">
    <ErrorState
      v-if="base.error.value"
      what="the starting revision"
      :error="base.error.value"
      compact
      @retry="base.reload()"
    />
    <TextField
      v-if="isNew"
      v-model="newPath"
      label="File path"
      mono
      required
      :disabled="busy"
      placeholder="docs/intro.md"
      :error="problem ?? undefined"
      hint="Folders in the path are created for you."
    />
    <TextArea
      v-model="text"
      :label="fileType === 'symlink' ? 'Link target' : isNew ? 'Contents' : `Contents of ${path}`"
      :rows="fileType === 'symlink' ? 3 : 18"
      :disabled="busy"
      mono
    />
    <div class="bar">
      <p class="faint">
        {{
          changed
            ? 'Nothing is saved to the branch yet. “Propose change” opens a change request for review.'
            : 'No changes yet.'
        }}
      </p>
      <div class="btns">
        <Button :disabled="!dirty" icon="download" @click="downloadDraft">Download draft</Button>
        <Button :disabled="busy" @click="cancel">Cancel</Button>
        <Button
          variant="primary"
          icon="proposal"
          :disabled="!canPropose"
          data-testid="propose-change"
          @click="ask"
          >Propose change…</Button
        >
      </div>
    </div>

    <Dialog
      :model-value="askingToLeave"
      title="Leave without proposing?"
      description="Your edits have not been sent to the server. Keep editing, or discard them to continue."
      size="sm"
      alert
      @update:model-value="decide(false)"
    >
      <p>You can download a copy of your draft before leaving.</p>
      <Button icon="download" @click="downloadDraft">Download draft</Button>
      <template #footer>
        <Button variant="danger" @click="decide(true)">Discard changes</Button>
        <Button variant="primary" data-autofocus @click="decide(false)">Keep editing</Button>
      </template>
    </Dialog>

    <Dialog
      v-model="asking"
      title="Propose this change"
      description="It opens a change request. Someone reviews it, then it lands on the branch."
      :dismissable="!busy"
    >
      <form id="propose-form" class="form" @submit.prevent="propose">
        <TextField
          v-model="title"
          label="Title"
          required
          autofocus
          hint="What this change does, in a few words."
        />
        <TextArea v-model="description" label="Description (optional)" :rows="3" />
        <p class="faint">
          For <code class="mono">{{ stream }}</code
          >, changing <code class="mono">{{ filePath }}</code
          >.
        </p>
        <Alert v-if="error" tone="bad" live="alert" title="Could not open the change request">
          {{ error.message }}
        </Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="asking = false">Back</Button>
        <Button
          type="submit"
          form="propose-form"
          variant="primary"
          :loading="busy"
          :disabled="!title.trim()"
          data-testid="propose-submit"
          >Open change request</Button
        >
      </template>
    </Dialog>
  </div>
</template>

<style scoped>
.editor,
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.btns {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
}
</style>
