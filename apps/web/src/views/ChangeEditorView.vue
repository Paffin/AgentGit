<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { describeError, isApiError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { useUnsavedChanges } from '@/composables/useUnsavedChanges'
import { batchChanges, fileStatus, restoreBatch, BATCH_BYTES, BATCH_LIMIT } from '@/lib/batch-edit'
import type { DraftFile, BatchDraft } from '@/lib/batch-edit'
import { pathProblem, proposeChange } from '@/lib/edit'
import { looksBinary, decodeUtf8 } from '@/lib/text'
import FileFinder from '@/components/files/FileFinder.vue'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'

const auth = useAuth()
const board = useBoard()
const route = useRoute()
const router = useRouter()
const { repo, routeParams } = useRepoName()
usePageTitle(() => `Edit files · ${repo.value}`)
const stream = computed(() =>
  typeof route.query.stream === 'string'
    ? route.query.stream
    : board.repo?.default_stream || 'main',
)
const files = ref<DraftFile[]>([])
const selected = ref('')
const active = computed(() => files.value.find((file) => file.id === selected.value))
const base = ref<string | null>(null)
const title = ref('')
const description = ref('')
const finding = ref(false)
const restoreInput = ref<HTMLInputElement | null>(null)
const pendingRestore = ref<BatchDraft | null>(null)
const newPath = ref('')
const error = ref('')
const needsSignIn = ref(false)
const storageError = ref('')
const blockedStorage = ref(false)
const restored = ref(false)
const busy = ref(false)
const reviewing = ref(false)
const startingOver = ref(false)
const submitted = ref(false)
const dirty = computed(
  () =>
    !submitted.value &&
    files.value.some((file) => !['Unchanged', 'Discarded'].includes(fileStatus(file))),
)
const changed = computed(() =>
  files.value.filter((file) => !['Unchanged', 'Discarded'].includes(fileStatus(file))),
)
const { askingToLeave, decide } = useUnsavedChanges(dirty, busy)
let context = 0
let key = ''
let restoring = false
let controller: AbortController | undefined
let downloadUrl = ''

function save() {
  if (restoring || !key || blockedStorage.value || submitted.value || base.value === null) return
  try {
    const draft: BatchDraft = {
      version: 1,
      base: base.value,
      files: files.value,
      title: title.value,
      description: description.value,
    }
    sessionStorage.setItem(key, JSON.stringify(draft))
    storageError.value = ''
  } catch {
    storageError.value = 'This tab could not save your draft. Download a copy before leaving.'
  }
}
async function load() {
  const mine = ++context
  controller?.abort()
  controller = new AbortController()
  restoring = true
  files.value = []
  selected.value = ''
  base.value = null
  title.value = ''
  description.value = ''
  error.value = ''
  needsSignIn.value = false
  reviewing.value = false
  startingOver.value = false
  finding.value = false
  pendingRestore.value = null
  storageError.value = ''
  blockedStorage.value = false
  restored.value = false
  submitted.value = false
  key = auth.identity
    ? `agentgit.batch.v1:${JSON.stringify([auth.identity, repo.value, stream.value])}`
    : ''
  if (!key) {
    restoring = false
    busy.value = false
    return
  }
  busy.value = true
  let readingDraft = true
  try {
    const saved = restoreBatch(sessionStorage.getItem(key))
    readingDraft = false
    if (saved) {
      applyDraft(saved)
    } else {
      const head = await api.streams.head(repo.value, stream.value, { signal: controller.signal })
      if (mine === context) base.value = head.head
    }
  } catch (cause) {
    if (mine !== context) return
    error.value = describeError(cause)
    blockedStorage.value = readingDraft
  } finally {
    if (mine === context) {
      busy.value = false
      restoring = false
    }
  }
}
watch(
  [repo, stream, () => auth.identity],
  () => {
    void load()
  },
  { immediate: true, flush: 'sync' },
)
watch([files, title, description], save, { deep: true, flush: 'sync' })
watch(
  () => auth.status,
  (status) => {
    if (status !== 'ready') {
      context++
      controller?.abort()
      busy.value = false
    } else if (controller?.signal.aborted) {
      void load()
    }
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
  if (downloadUrl) URL.revokeObjectURL(downloadUrl)
})

function addFile() {
  if (busy.value || base.value === null || reviewing.value) return
  const path = newPath.value.trim()
  const problem = pathProblem(path)
  if (problem) {
    error.value = problem
    return
  }
  if (files.value.some((file) => !file.removed && file.path === path)) {
    error.value = 'That file is already in this change.'
    return
  }
  if (files.value.length >= BATCH_LIMIT) {
    error.value = 'This change already has 100 files.'
    return
  }
  const file: DraftFile = {
    id: crypto.randomUUID(),
    originalPath: null,
    path,
    original: '',
    text: '',
    executable: false,
    fileType: 'regular',
    removed: false,
  }
  files.value.push(file)
  selected.value = file.id
  newPath.value = ''
  error.value = ''
}
async function openFile(path: string) {
  const existing = files.value.find((file) => file.originalPath === path)
  if (existing) {
    selected.value = existing.id
    return
  }
  if (!base.value || busy.value || files.value.length >= BATCH_LIMIT) return
  const mine = context
  busy.value = true
  error.value = ''
  try {
    const [location, bytes] = await Promise.all([
      api.tree.resolve(repo.value, base.value, path, { signal: controller?.signal }),
      api.tree.file(repo.value, base.value, path, BATCH_BYTES, { signal: controller?.signal }),
    ])
    if (mine !== context) return
    if (bytes.truncated) throw new Error('This file exceeds the 4 MiB editing limit.')
    if (looksBinary(bytes.bytes))
      throw new Error('This file is binary. Use Upload files to replace binary content.')
    const text = decodeUtf8(bytes.bytes)
    const file: DraftFile = {
      id: crypto.randomUUID(),
      originalPath: path,
      path,
      original: text,
      text,
      executable: location.entry?.executable === true,
      fileType: location.entry?.file_type === 'symlink' ? 'symlink' : 'regular',
      removed: false,
    }
    files.value.push(file)
    selected.value = file.id
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  } finally {
    if (mine === context) busy.value = false
  }
}
async function review() {
  if (busy.value || base.value === null) return
  const mine = context
  busy.value = true
  error.value = ''
  try {
    const changes = batchChanges(files.value)
    if (!changes.length) throw new Error('Change a file before opening a change request.')
    const deleted = new Set(
      changes.filter((change) => change.op === 'delete').map((change) => change.path),
    )
    for (const file of changed.value.filter(
      (file) => !file.removed && file.path !== file.originalPath,
    )) {
      if (!base.value || deleted.has(file.path)) continue
      let exists = false
      try {
        const location = await api.tree.resolve(repo.value, base.value, file.path, {
          signal: controller?.signal,
        })
        exists = location.kind !== 'missing'
      } catch (cause) {
        if (!isApiError(cause) || !cause.isNotFound) throw cause
      }
      if (exists)
        throw new Error(`${file.path} already exists. Open it for editing or choose another path.`)
    }
    if (mine !== context) return
    if (!title.value.trim())
      title.value = `Update ${changed.value.length} file${changed.value.length === 1 ? '' : 's'}`
    reviewing.value = true
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  } finally {
    if (mine === context) busy.value = false
  }
}
async function submit() {
  if (
    busy.value ||
    !title.value.trim() ||
    base.value === null ||
    auth.status !== 'ready' ||
    needsSignIn.value
  )
    return
  const mine = context
  busy.value = true
  error.value = ''
  try {
    const proposal = await proposeChange(api, {
      repo: repo.value,
      stream: stream.value,
      fromRev: base.value || undefined,
      expectedBase: base.value,
      changes: batchChanges(files.value),
      title: title.value.trim(),
      description: description.value.trim(),
      request: { token: auth.token, quietUnauthenticated: true, signal: controller?.signal },
    })
    if (mine !== context) return
    submitted.value = true
    try {
      sessionStorage.removeItem(key)
    } catch {
      /* the completed draft can be discarded when reopened */
    }
    busy.value = false
    board.touched(['proposals', 'streams', 'predict'])
    await router.push({ name: 'proposal', params: { ...routeParams.value, id: proposal.id } })
  } catch (cause) {
    if (mine === context) {
      error.value = describeError(cause)
      needsSignIn.value = isApiError(cause) && cause.isUnauthenticated
    }
  } finally {
    if (mine === context) busy.value = false
  }
}
function resetDraft() {
  try {
    sessionStorage.removeItem(key)
    startingOver.value = false
    void load()
  } catch {
    storageError.value = 'The saved draft could not be removed.'
  }
}
function applyDraft(draft: BatchDraft) {
  files.value = draft.files
  title.value = draft.title
  description.value = draft.description
  base.value = draft.base
  selected.value = draft.files[0]?.id || ''
  restored.value = true
  reviewing.value = false
}
function confirmRestore() {
  if (!pendingRestore.value || busy.value) return
  restoring = true
  applyDraft(pendingRestore.value)
  pendingRestore.value = null
  blockedStorage.value = false
  error.value = ''
  restoring = false
  save()
}
async function readDraftFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file || busy.value) return
  const mine = context
  try {
    if (file.size > BATCH_BYTES * 3) throw new Error('This draft is too large to restore.')
    const raw = await file.text()
    if (mine !== context) return
    const envelope = JSON.parse(raw)
    if (envelope?.repository !== repo.value || envelope?.stream !== stream.value) {
      throw new Error('Open the repository and branch named in this draft before restoring it.')
    }
    const draft = restoreBatch(raw)
    if (!draft) throw new Error('This file does not contain a change draft.')
    pendingRestore.value = draft
    if (!dirty.value && !title.value.trim() && !description.value.trim()) confirmRestore()
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  }
}
function download() {
  try {
    const content = blockedStorage.value
      ? sessionStorage.getItem(key)
      : JSON.stringify(
          {
            repository: repo.value,
            stream: stream.value,
            version: 1,
            base: base.value,
            files: files.value,
            title: title.value,
            description: description.value,
          },
          null,
          2,
        )
    if (!content) throw new Error('There is no saved draft to download.')
    if (downloadUrl) URL.revokeObjectURL(downloadUrl)
    downloadUrl = URL.createObjectURL(new Blob([content], { type: 'application/json' }))
    const link = document.createElement('a')
    link.href = downloadUrl
    link.download = 'agentgit-change-draft.json'
    document.body.appendChild(link)
    link.click()
    link.remove()
  } catch (cause) {
    storageError.value = describeError(cause)
  }
}
</script>

<template>
  <div class="change-editor">
    <header class="page-heading">
      <div>
        <h1>Edit files</h1>
        <p class="muted">
          One change request for all your edits on <code>{{ stream }}</code
          >. Nothing lands until it is reviewed.
        </p>
      </div>
      <div class="header-actions">
        <input
          ref="restoreInput"
          class="restore-input"
          type="file"
          accept="application/json,.json"
          aria-label="Saved change draft"
          @change="readDraftFile"
        />
        <Button :disabled="busy || !auth.can(repo, 'editFiles')" @click="restoreInput?.click()"
          >Restore draft</Button
        >
        <Button :disabled="!files.length && !blockedStorage" @click="download"
          >Download draft</Button
        ><Button :disabled="busy" @click="startingOver = true">Start over</Button>
      </div>
    </header>
    <EmptyState
      v-if="!auth.can(repo, 'editFiles')"
      icon="lock"
      title="Writing access required"
      description="Ask a repository administrator for permission to propose file changes."
    />
    <template v-else>
      <p class="muted">
        Edit up to 100 text files or link targets, with 4 MiB of changed content per request.
      </p>
      <Alert v-if="error" tone="bad" live="alert" title="Could not complete the action"
        >{{ error }} <Button v-if="base === null" @click="load">Try again</Button></Alert
      >
      <Alert v-if="storageError" tone="warn" title="Draft storage">{{ storageError }}</Alert>
      <p v-if="restored" class="muted" role="status">
        Draft recovered with its original starting revision.
      </p>
      <p v-else class="muted">
        Your draft is saved in this tab. Download it before closing the tab.
      </p>
      <div v-if="!reviewing" class="editor-layout">
        <aside aria-label="Files in this change" class="file-list">
          <Button :disabled="busy || !base" @click="finding = true">Open existing file</Button>
          <form class="new-file" @submit.prevent="addFile">
            <TextField
              v-model="newPath"
              label="New file path"
              placeholder="src/example.ts"
              :disabled="busy || base === null"
            /><Button type="submit" :disabled="busy || base === null">Add file</Button>
          </form>
          <ul>
            <li v-for="file in files" :key="file.id">
              <button
                type="button"
                :disabled="busy"
                :aria-current="selected === file.id ? 'true' : undefined"
                :title="file.path"
                @click="selected = file.id"
              >
                <span class="mono">{{ file.path }}</span
                ><small>{{ fileStatus(file) }}</small>
              </button>
            </li>
          </ul>
        </aside>
        <section v-if="active" class="file-content" aria-label="File editor">
          <TextField
            v-model="active.path"
            label="File path"
            mono
            :disabled="busy || active.removed"
            hint="Change the path to rename or move this file."
          />
          <div class="file-actions">
            <span class="muted"
              >{{ fileStatus(active)
              }}<template v-if="active.executable"> · executable</template></span
            ><Button :disabled="busy" @click="active.removed = !active.removed">{{
              active.removed ? 'Restore file' : active.originalPath ? 'Delete file' : 'Discard file'
            }}</Button>
          </div>
          <TextArea
            v-if="!active.removed"
            v-model="active.text"
            :label="active.fileType === 'symlink' ? 'Link target' : 'Contents'"
            mono
            :rows="24"
            :disabled="busy"
          />
          <EmptyState
            v-else
            icon="file"
            title="File removed from the proposed result"
            description="Restore it to keep editing. The branch is unchanged."
          />
        </section>
        <EmptyState
          v-else
          icon="file"
          title="Start with a file"
          description="Open an existing text file or add a new file. You can edit, rename and delete several files together."
        />
      </div>
      <section v-else class="review" aria-label="Review changes">
        <h2>Review {{ changed.length }} changed files</h2>
        <p class="muted">
          These are your staged contents. The server checks them against the original revision when
          you submit.
        </p>
        <details v-for="(file, index) in changed" :key="file.id" :open="index === 0">
          <summary>
            <strong>{{ file.path }}</strong> · {{ fileStatus(file)
            }}<span v-if="file.originalPath && file.originalPath !== file.path">
              from {{ file.originalPath }}</span
            >
          </summary>
          <div class="comparison">
            <section v-if="file.originalPath">
              <h3>Before</h3>
              <pre tabindex="0">{{ file.original }}</pre>
            </section>
            <section v-if="!file.removed">
              <h3>After</h3>
              <pre tabindex="0">{{ file.text || '(empty file)' }}</pre>
            </section>
          </div>
        </details>
        <TextField
          v-model="title"
          label="Change request title"
          :disabled="busy"
          required
        /><TextArea
          v-model="description"
          label="Description (optional)"
          :disabled="busy"
          :rows="3"
        />
      </section>
      <footer class="editor-footer">
        <span>{{ changed.length }} changed · {{ files.length }} files in draft</span>
        <div>
          <Button v-if="reviewing" :disabled="busy" @click="reviewing = false"
            >Back to editing</Button
          ><Button
            v-if="reviewing"
            variant="primary"
            :loading="busy"
            :disabled="!title.trim() || needsSignIn"
            @click="submit"
            >Open change request</Button
          ><Button
            v-else
            variant="primary"
            :loading="busy"
            :disabled="!changed.length || base === null"
            @click="review"
            >Review changes</Button
          >
        </div>
      </footer>
      <FileFinder v-model="finding" :repo="repo" :rev="base || stream" @select="openFile" />
    </template>
    <Dialog
      :model-value="pendingRestore !== null"
      title="Replace this draft?"
      description="This replaces the edits in this tab with the saved draft and its original starting revision. The branch is unchanged."
      @update:model-value="pendingRestore = null"
    >
      <template #footer
        ><Button @click="pendingRestore = null">Keep current draft</Button
        ><Button variant="primary" @click="confirmRestore">Restore saved draft</Button></template
      >
    </Dialog>
    <Dialog
      v-model="startingOver"
      title="Discard this saved draft?"
      description="Only this account's draft for this repository and branch is removed. The branch and change requests are unchanged."
      alert
      ><template #footer
        ><Button @click="startingOver = false">Keep draft</Button
        ><Button variant="danger" @click="resetDraft">Discard draft</Button></template
      ></Dialog
    >
    <Dialog
      :model-value="askingToLeave"
      title="Leave this change?"
      description="Your draft stays in this tab. Download a copy before closing the tab."
      @update:model-value="decide(false)"
      ><template #footer
        ><Button @click="decide(false)">Keep editing</Button
        ><Button @click="decide(true)">Keep draft and leave</Button></template
      ></Dialog
    >
  </div>
</template>

<style scoped>
.restore-input {
  display: none;
}
.change-editor {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.header-actions,
.page-heading,
.file-actions,
.editor-footer {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.page-heading h1 {
  margin-bottom: var(--s-2);
}
.editor-layout {
  display: grid;
  grid-template-columns: minmax(190px, 240px) minmax(0, 1fr);
  gap: var(--s-4);
  align-items: start;
}
.file-list {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  border-right: 1px solid var(--border);
  padding-right: var(--s-4);
}
.new-file {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.file-list ul {
  list-style: none;
  padding: 0;
  margin: 0;
}
.file-list li + li {
  border-top: 1px solid var(--border);
}
.file-list li button {
  display: flex;
  flex-direction: column;
  align-items: start;
  gap: var(--s-1);
  width: 100%;
  padding: var(--s-3);
  border: 0;
  background: transparent;
  color: inherit;
  text-align: left;
  cursor: pointer;
}
.file-list li button[aria-current='true'] {
  background: var(--surface-2);
  box-shadow: inset 3px 0 var(--accent);
}
.file-list .mono {
  overflow-wrap: anywhere;
}
.file-list small {
  color: var(--text-3);
}
.file-content,
.review {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  min-width: 0;
}
.editor-footer {
  border-top: 1px solid var(--border);
  padding-top: var(--s-3);
}
.editor-footer > div {
  display: flex;
  gap: var(--s-2);
}
.review details {
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: var(--s-3);
}
.review summary {
  cursor: pointer;
  overflow-wrap: anywhere;
}
.comparison {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: var(--s-3);
}
.comparison section {
  min-width: 0;
}
.comparison h3 {
  font-size: var(--fs-sm);
  color: var(--text-3);
  margin: var(--s-3) 0 var(--s-2);
}
.comparison pre {
  overflow: auto;
  max-height: 24rem;
  padding: var(--s-3);
  background: var(--surface-2);
  font: var(--fs-sm)/1.6 var(--font-mono);
}
@media (max-width: 700px) {
  .editor-layout {
    grid-template-columns: 1fr;
  }
  .file-list {
    border-right: 0;
    padding-right: 0;
    border-bottom: 1px solid var(--border);
    padding-bottom: var(--s-3);
  }
  .file-list ul {
    max-height: 12rem;
    overflow: auto;
  }
  .comparison {
    grid-template-columns: 1fr;
  }
}
</style>
