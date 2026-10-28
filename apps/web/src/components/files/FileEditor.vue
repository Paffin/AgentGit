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
import { useFileDraft } from '@/composables/useFileDraft'
import { useAuth } from '@/stores/auth'
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
const auth = useAuth()
let submission = 0
let controller: AbortController | undefined
function invalidateSubmission() {
  submission++
  controller?.abort()
  busy.value = false
}
onBeforeUnmount(invalidateSubmission)
watch(
  [() => auth.identity, () => props.repo, () => props.path, () => props.stream],
  () => {
    invalidateSubmission()
    // Let sign-out finish its existing leave/draft confirmation before unmounting.
    if (auth.identity) emit('cancel')
  },
  { flush: 'sync' },
)
watch(
  () => auth.status,
  (status) => {
    if (status !== 'ready') invalidateSubmission()
  },
  { flush: 'sync' },
)
const draft = useFileDraft({
  identity: auth.identity,
  repo: props.repo,
  stream: props.stream,
  path: props.path,
  isNew: !!props.isNew,
})
const recovered = draft.initial
const originalText = recovered?.original ?? props.original
const executable = recovered?.executable ?? props.executable === true
const fileType = recovered?.fileType ?? (props.fileType === 'symlink' ? 'symlink' : 'regular')

const initialPath = props.isNew ? (props.path ? `${props.path}/` : '') : props.path
const newPath = ref(recovered?.path ?? initialPath)
const text = ref(recovered?.text ?? props.original)
const filePath = computed(() => (props.isNew ? newPath.value.trim() : props.path))
const problem = computed(() => (props.isNew && newPath.value ? pathProblem(newPath.value) : null))
const changed = computed(() => (props.isNew ? filePath.value !== '' : text.value !== originalText))
const base = useResource(async () =>
  recovered
    ? recovered.base
    : props.isNew
      ? (await api.streams.head(props.repo, props.stream)).head
      : props.rev,
)
const canPropose = computed(
  () =>
    auth.status === 'ready' &&
    changed.value &&
    base.settled.value &&
    !base.error.value &&
    !(props.isNew && pathProblem(newPath.value) !== null),
)

const asking = ref(false)
const title = ref(recovered?.title ?? '')
const description = ref(recovered?.description ?? '')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const submitted = ref(false)
const dirty = computed(
  () => !submitted.value && (text.value !== originalText || newPath.value !== initialPath),
)
const replaceDraft = ref(false)
function persistDraft() {
  if (submitted.value) {
    draft.clear()
    return
  }
  if (!dirty.value) {
    if (!draft.blocked.value) draft.clear()
    return
  }
  if (!base.settled.value || base.error.value || base.data.value === undefined) return
  draft.save({
    text: text.value,
    original: originalText,
    path: newPath.value,
    base: base.data.value,
    executable,
    fileType,
    title: title.value,
    description: description.value,
  })
}
watch([text, newPath, title, description, base.data, base.settled, submitted], persistDraft, {
  flush: 'sync',
})
const { askingToLeave, requestLeave, decide } = useUnsavedChanges(dirty, busy)
function discardAndLeave() {
  if (draft.clear()) decide(true)
}
function confirmReplaceDraft() {
  if (!draft.clear()) return
  replaceDraft.value = false
  persistDraft()
}
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
  if (!title.value) title.value = props.isNew ? `Add ${filePath.value}` : `Update ${filePath.value}`
  error.value = null
  asking.value = true
}

async function propose() {
  if (busy.value || !canPropose.value || !title.value.trim() || error.value?.isUnauthenticated)
    return
  const mine = ++submission
  const current = new AbortController()
  controller = current
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
          executable,
          fileType === 'symlink' ? 'symlink' : undefined,
        ),
      ],
      title: title.value.trim(),
      description: description.value.trim() || undefined,
      request: { token: auth.token, quietUnauthenticated: true, signal: current.signal },
    })
    if (mine !== submission || current.signal.aborted) return
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
    if (mine !== submission || current.signal.aborted) return
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    if (mine === submission) busy.value = false
  }
}
</script>

<template>
  <div class="editor" data-testid="file-editor">
    <Alert v-if="draft.restored.value" tone="info" title="Draft recovered">
      Your text and its original starting revision were recovered from this tab.
      <p v-if="recovered && recovered.base !== props.rev && !isNew">
        The branch has moved since this draft started. Its original base is preserved so concurrent
        changes can be checked during review.
      </p>
    </Alert>
    <Alert v-if="draft.error.value" tone="warn" title="Draft needs attention" live="alert">
      {{ draft.error.value }}
      <Button v-if="draft.blocked.value" @click="replaceDraft = true"
        >Replace the unreadable draft</Button
      >
    </Alert>
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
        <span v-if="draft.saved.value" role="status" data-testid="draft-saved">
          Draft saved in this tab. It survives reload and sign-in here; download it before closing
          the tab.</span
        >
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
      <Alert v-if="draft.error.value" tone="warn" title="Draft storage">{{
        draft.error.value
      }}</Alert>
      <Button icon="download" @click="downloadDraft">Download draft</Button>
      <template #footer>
        <Button variant="danger" @click="discardAndLeave">Discard changes</Button>
        <Button v-if="draft.saved.value" @click="decide(true)">Keep draft and leave</Button>
        <Button variant="primary" data-autofocus @click="decide(false)">Keep editing</Button>
      </template>
    </Dialog>

    <Dialog v-model="replaceDraft" title="Replace the unreadable draft?" size="sm" alert>
      <p>
        The stored copy will be removed and replaced with the text currently in this editor.
        Download any text you want to keep first.
      </p>
      <template #footer>
        <Button @click="replaceDraft = false">Cancel</Button>
        <Button variant="danger" @click="confirmReplaceDraft">Replace draft</Button>
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
          :disabled="busy"
          required
          autofocus
          hint="What this change does, in a few words."
        />
        <TextArea v-model="description" label="Description (optional)" :rows="3" :disabled="busy" />
        <p class="faint">
          For <code class="mono">{{ stream }}</code
          >, changing <code class="mono">{{ filePath }}</code
          >.
        </p>
        <Alert v-if="error" tone="bad" live="alert" title="Could not open the change request">
          {{ error.message }}
          <Button
            v-if="error.isUnauthenticated"
            :to="{ path: '/signin', query: { next: router.currentRoute.value.fullPath } }"
            >Sign in again</Button
          >
        </Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="asking = false">Back</Button>
        <Button
          type="submit"
          form="propose-form"
          variant="primary"
          :loading="busy"
          :disabled="!title.trim() || !canPropose || error?.isUnauthenticated"
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
