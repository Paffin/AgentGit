<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import { pathProblem, proposeChange, putFile, toBase64 } from '@/lib/edit'
import { decodeUtf8, looksBinary, preserveTextFormat } from '@/lib/text'
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
const originalText = ref(recovered?.original ?? props.original)
const executable = ref(recovered?.executable ?? props.executable === true)
const fileType = ref(recovered?.fileType ?? (props.fileType === 'symlink' ? 'symlink' : 'regular'))

const initialPath = props.isNew ? (props.path ? `${props.path}/` : '') : props.path
const newPath = ref(recovered?.path ?? initialPath)
const text = ref(recovered?.text ?? props.original)
const filePath = computed(() => (props.isNew ? newPath.value.trim() : props.path))
const problem = computed(() => (props.isNew && newPath.value ? pathProblem(newPath.value) : null))
const changed = computed(() =>
  props.isNew ? filePath.value !== '' : text.value !== originalText.value,
)
const base = useResource(async () =>
  recovered
    ? recovered.base
    : props.isNew
      ? (await api.streams.head(props.repo, props.stream)).head
      : props.rev,
)
const restoredBase = ref<string | null>(null)
const startingBase = computed(() => restoredBase.value ?? base.data.value)
const accountSupported = computed(
  () => !props.isNew && !!auth.capabilities?.features.includes('files.drafts'),
)
type AccountFileDraft = Awaited<ReturnType<typeof api.files.draft>>
const accountAvailable = ref<AccountFileDraft | null>(null)
const accountBusy = ref(false)
const accountLoaded = ref(false)
const accountError = ref('')
const accountRestored = ref(false)
let accountVersion = 0
let adoptedVersion: number | null = null
let restoringAccount = false

const canPropose = computed(
  () =>
    auth.status === 'ready' &&
    !accountBusy.value &&
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
  () => !submitted.value && (text.value !== originalText.value || newPath.value !== initialPath),
)
const replaceDraft = ref(false)
function persistDraft() {
  if (restoringAccount) return
  if (submitted.value) {
    draft.clear()
    return
  }
  if (!dirty.value) {
    if (!draft.blocked.value) draft.clear()
    return
  }
  if (!base.settled.value || base.error.value || startingBase.value === undefined) return
  draft.save({
    text: text.value,
    original: originalText.value,
    path: newPath.value,
    base: startingBase.value,
    executable: executable.value,
    fileType: fileType.value,
    title: title.value,
    description: description.value,
  })
}
watch([text, newPath, title, description, startingBase, base.settled, submitted], persistDraft, {
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
  draftUrl = URL.createObjectURL(
    new Blob([preserveTextFormat(originalText.value, text.value)], {
      type: 'text/plain;charset=utf-8',
    }),
  )
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

async function restoreAccountDraft(saved: AccountFileDraft) {
  const mine = submission
  const token = auth.token
  accountBusy.value = true
  accountError.value = ''
  try {
    const original = await api.tree.file(props.repo, saved.base, props.path, 4 * 1024 * 1024)
    const bytes = Uint8Array.from(atob(saved.content), (char) => char.charCodeAt(0))
    if (original.truncated || looksBinary(original.bytes) || looksBinary(bytes))
      throw new Error(
        'This account draft cannot be opened in the text editor. Use the CLI to retrieve it.',
      )
    if (mine !== submission || token !== auth.token) return
    restoringAccount = true
    originalText.value = decodeUtf8(original.bytes)
    executable.value = saved.executable
    fileType.value = saved.file_type === 'symlink' ? 'symlink' : 'regular'
    restoredBase.value = saved.base
    text.value = decodeUtf8(bytes)
    adoptedVersion = saved.version
    accountAvailable.value = null
    accountRestored.value = true
    restoringAccount = false
    persistDraft()
  } catch (cause) {
    if (mine === submission && token === auth.token) accountError.value = describeError(cause)
  } finally {
    restoringAccount = false
    if (mine === submission) accountBusy.value = false
  }
}
async function loadAccountDraft() {
  if (!accountSupported.value || auth.status !== 'ready') return
  const mine = submission
  const token = auth.token
  accountBusy.value = true
  accountLoaded.value = false
  accountError.value = ''
  adoptedVersion = null
  try {
    const saved = await api.files.draft(props.repo, props.stream, props.path, { token })
    if (mine !== submission || token !== auth.token) return
    accountVersion = saved.version
    accountLoaded.value = true
    accountAvailable.value = null
    if (!saved.present) return
    accountAvailable.value = saved
    if (!dirty.value) await restoreAccountDraft(saved)
  } catch (cause) {
    if (mine === submission && token === auth.token) accountError.value = describeError(cause)
  } finally {
    if (mine === submission) accountBusy.value = false
  }
}
function keepTabDraft() {
  accountAvailable.value = null
  adoptedVersion = null
}
async function saveAccountDraft() {
  if (
    !accountSupported.value ||
    !accountLoaded.value ||
    accountBusy.value ||
    busy.value ||
    submitted.value ||
    accountAvailable.value ||
    !startingBase.value
  )
    return
  const content = preserveTextFormat(originalText.value, text.value)
  if (new TextEncoder().encode(content).length > 4 * 1024 * 1024) {
    accountError.value =
      'Account file drafts are limited to 4 MiB. Download this draft to keep a copy.'
    return
  }
  const mine = submission
  const token = auth.token
  accountBusy.value = true
  accountError.value = ''
  try {
    const saved = await api.files.saveDraft(
      props.repo,
      {
        stream: props.stream,
        path: props.path,
        base: startingBase.value,
        content: toBase64(content),
        expected_version: accountVersion,
      },
      { token, quietUnauthenticated: true },
    )
    if (mine !== submission || token !== auth.token) return
    accountVersion = saved.version
    adoptedVersion = saved.version
    toasts.ok('File draft saved to your account')
  } catch (cause) {
    if (mine !== submission || token !== auth.token) return
    accountError.value = describeError(cause)
    if (cause instanceof ApiError && cause.code === 'failed_precondition')
      accountLoaded.value = false
  } finally {
    if (mine === submission) accountBusy.value = false
  }
}
watch(
  [accountSupported, () => auth.status],
  () => {
    void loadAccountDraft()
  },
  { immediate: true },
)

function ask() {
  if (!title.value) title.value = props.isNew ? `Add ${filePath.value}` : `Update ${filePath.value}`
  error.value = null
  asking.value = true
}

async function propose() {
  if (
    busy.value ||
    submitted.value ||
    !canPropose.value ||
    !title.value.trim() ||
    error.value?.isUnauthenticated
  )
    return
  const mine = ++submission
  const cleanupVersion = adoptedVersion
  const token = auth.token
  const current = new AbortController()
  controller = current
  busy.value = true
  error.value = null
  try {
    const p = await proposeChange(api, {
      repo: props.repo,
      stream: props.stream,
      fromRev: startingBase.value || undefined,
      expectedBase: startingBase.value,
      changes: [
        putFile(
          filePath.value,
          preserveTextFormat(originalText.value, text.value),
          executable.value,
          fileType.value === 'symlink' ? 'symlink' : undefined,
        ),
      ],
      title: title.value.trim(),
      description: description.value.trim() || undefined,
      request: { token, quietUnauthenticated: true, signal: current.signal },
    })
    if (mine !== submission || current.signal.aborted) return
    submitted.value = true
    asking.value = false
    board.touched(['proposals', 'streams', 'predict'])
    toasts.ok('Change request opened', p.title)
    if (cleanupVersion !== null) {
      try {
        await api.files.clearDraft(props.repo, props.stream, props.path, cleanupVersion, {
          token,
          signal: current.signal,
          quietUnauthenticated: true,
        })
      } catch (cause) {
        if (mine === submission) toasts.warn('Account draft kept', describeError(cause))
      }
      if (mine !== submission || current.signal.aborted) return
    }
    busy.value = false
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
    <Alert v-if="accountAvailable" tone="info" title="An account file draft is available">
      Your tab draft is unchanged. Choose which copy to continue.
      <div class="btns">
        <Button :disabled="busy || accountBusy" @click="restoreAccountDraft(accountAvailable)"
          >Restore account draft</Button
        >
        <Button :disabled="busy || accountBusy" @click="keepTabDraft">Keep tab draft</Button>
      </div>
    </Alert>
    <p v-else-if="accountRestored" class="faint" role="status">
      Account draft restored with its original starting revision.
    </p>
    <Alert v-if="accountError" tone="warn">
      {{ accountError }}
      <Button :disabled="busy || accountBusy" @click="loadAccountDraft"
        >Reload account draft</Button
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
      :disabled="busy || accountBusy"
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
          {{
            accountSupported
              ? 'Draft saved in this tab. Save to account to continue on another device.'
              : 'Draft saved in this tab. It survives reload and sign-in here; download it before closing the tab.'
          }}</span
        >
      </p>
      <div class="btns">
        <Button
          v-if="accountSupported"
          :loading="accountBusy"
          :disabled="busy || !changed || !accountLoaded || !!accountAvailable"
          @click="saveAccountDraft"
          >Save to account</Button
        >
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
      description="Your edits have not been proposed. Keep editing, or discard this tab’s copy to continue."
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
