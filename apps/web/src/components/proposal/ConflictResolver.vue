<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Conflict, Proposal, SyncWorkspaceResponse } from '@/api/types'
import { canResolveHere, workspaceName } from '@/lib/edit'
import { conflictLabel } from '@/lib/conflict-label'
import { formatBytes } from '@/lib/format'
import {
  structuralRoots,
  within,
  scopePaths,
  loadStructuralSide,
  structuralChanges,
} from '@/lib/structuralResolution'
import type { TreeEntry } from '@/api/types'
import { loadConflictVersion, conflictChange } from '@/lib/conflictResolution'
import type { ConflictChoice, ConflictVersion } from '@/lib/conflictResolution'
import { useToasts } from '@/stores/toasts'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextArea from '@/ui/TextArea.vue'
import CopyButton from '@/ui/CopyButton.vue'

const props = defineProps<{ modelValue: boolean; repo: string; proposal: Proposal }>()
const emit = defineEmits<{ 'update:modelValue': [open: boolean]; done: [] }>()
const toasts = useToasts()
const auth = useAuth()
interface Structural {
  paths: string[]
  mine: TreeEntry[]
  theirs: TreeEntry[]
}
interface Row {
  structural?: Structural
  conflict: Conflict
  mine: ConflictVersion
  theirs: ConflictVersion
  blocked: string
  editable: boolean
  choice: ConflictChoice
  editSource: 'mine' | 'theirs'
  text: string
}
interface Attempt {
  repo: string
  proposal: string
  stream: string
  head: string
  branchHead: string
  workspace: string
  token: string | null
  structuralBudget: { remaining: number }
  controller: AbortController
  created: boolean
  released: boolean
}
const phase = ref<'idle' | 'loading' | 'ready' | 'saving' | 'error'>('idle')
const rows = ref<Row[]>([])
const error = ref<ApiError | null>(null)
let active: Attempt | undefined
const asError = (e: unknown) =>
  e instanceof ApiError
    ? e
    : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
const request = (a: Attempt) => ({
  token: a.token,
  signal: a.controller.signal,
  quietUnauthenticated: true,
})
const current = (a: Attempt) => active === a && !a.controller.signal.aborted
const absent = (): ConflictVersion => ({ entry: null, text: null })

async function release(a: Attempt) {
  if (!a.created || a.released) return
  a.released = true
  try {
    await api.workspaces.remove(a.repo, a.workspace, {
      token: a.token,
      quietUnauthenticated: true,
      timeoutMs: 5000,
    })
  } catch {
    // The private workspace remains visible to its original owner if cleanup fails.
  }
}
function invalidate() {
  const previous = active
  active = undefined
  previous?.controller.abort()
  if (previous) void release(previous)
  phase.value = 'idle'
}
function sourcePath(conflict: Conflict, side: 'ours' | 'theirs') {
  return conflict.origin?.[side] ?? conflict.path
}
async function prepareRow(a: Attempt, conflict: Conflict): Promise<Row> {
  const row: Row = {
    conflict,
    mine: absent(),
    theirs: absent(),
    blocked: '',
    editable: false,
    choice: '',
    editSource: 'mine',
    text: '',
  }
  if (conflict.kind === 'file_directory') return prepareStructural(a, row)
  if (!canResolveHere(conflict.kind)) {
    row.blocked = `${conflictLabel(conflict)}: resolve this structural conflict with the command line.`
    return row
  }
  ;[row.mine, row.theirs] = await Promise.all([
    loadConflictVersion(api, a.repo, a.head, sourcePath(conflict, 'ours'), request(a)),
    loadConflictVersion(api, a.repo, a.branchHead, sourcePath(conflict, 'theirs'), request(a)),
  ])
  row.editable = [row.mine, row.theirs].every(
    (v) => !v.entry || (v.entry.file_type === 'regular' && v.text !== null),
  )
  row.editSource = row.mine.entry ? 'mine' : 'theirs'
  row.text = row[row.editSource].text ?? ''
  if (row.editable) row.choice = row.editSource
  return row
}
async function prepareStructural(a: Attempt, row: Row): Promise<Row> {
  try {
    if (!auth.capabilities?.features.includes('workspaces.structural_preview'))
      throw new Error('Upgrade the server to choose a complete file or folder.')
    const paths = scopePaths(row.conflict.path, row.conflict.resolution_scope)
    const [mine, theirs] = await Promise.all([
      loadStructuralSide(api, a.repo, a.head, row.conflict.path, request(a), a.structuralBudget),
      loadStructuralSide(
        api,
        a.repo,
        a.branchHead,
        row.conflict.path,
        request(a),
        a.structuralBudget,
      ),
    ])
    if (new Set([...mine, ...theirs].map((entry) => entry.path)).size > 100)
      throw new Error('This folder is too large to resolve here. Use the command line.')
    row.structural = { paths, mine, theirs }
  } catch (cause) {
    row.blocked = describeError(cause)
  }
  return row
}
function unsupportedChildren(conflicts: Conflict[], root: string) {
  return conflicts.some(
    (child) =>
      within(root, child.path) && child.kind !== 'file_directory' && !canResolveHere(child.kind),
  )
}
function scopeWithChildren(conflict: Conflict, conflicts: Conflict[]): Conflict {
  try {
    const paths = scopePaths(conflict.path, conflict.resolution_scope)
    const covered = conflicts
      .filter((child) => within(conflict.path, child.path))
      .map((child) => child.path)
    return {
      ...conflict,
      resolution_scope: { complete: true, paths: [...new Set([...paths, ...covered])] },
    }
  } catch {
    return conflict
  }
}
function mergeRows(conflicts: Conflict[]): Conflict[] {
  const roots = structuralRoots(conflicts)
  const total = conflicts.reduce(
    (sum, conflict) => sum + (conflict.resolution_scope?.paths.length ?? 0),
    0,
  )
  return conflicts
    .filter(
      (conflict) =>
        !roots.some(
          (root) =>
            within(root, conflict.path) &&
            (root !== conflict.path || conflict.kind !== 'file_directory'),
        ),
    )
    .map((conflict) => {
      if (!roots.includes(conflict.path)) return conflict
      const unsupported = unsupportedChildren(conflicts, conflict.path)
      if (total > 1000 || unsupported) return { ...conflict, resolution_scope: null }
      return scopeWithChildren(conflict, conflicts)
    })
}
function rowChanges(row: Row) {
  if (row.structural) {
    const selected = row.choice === 'mine' ? row.structural.mine : row.structural.theirs
    const other = row.choice === 'mine' ? row.structural.theirs : row.structural.mine
    return structuralChanges(row.structural.paths, selected, other)
  }
  return [
    conflictChange(row.conflict.path, row.choice, row.mine, row.theirs, row.text, row.editSource),
  ]
}
function fileCount(entries: TreeEntry[]) {
  return String(entries.length) + (entries.length === 1 ? ' file' : ' files')
}
function structuralLabel(side: string, entries: TreeEntry[]) {
  return entries.length ? `Keep ${side} (${fileCount(entries)})` : `Remove path (absent in ${side})`
}
function removedPaths(row: Row) {
  if (!row.structural || !row.choice) return []
  const selected = row.choice === 'mine' ? row.structural.mine : row.structural.theirs
  return row.structural.paths.filter((path) => !selected.some((entry) => entry.path === path))
}
async function moveProposal(a: Attempt, head: string) {
  a.controller.signal.throwIfAborted()
  if (!head) throw new Error('The merge did not return a revision.')
  await api.proposals.update(a.repo, a.proposal, head, a.head, request(a))
  if (!current(a)) return
  toasts.ok('Conflicts resolved', 'The change request now includes the inspected branch revision.')
  void release(a)
  emit('update:modelValue', false)
  emit('done')
}
async function prepareMerge(a: Attempt, result: SyncWorkspaceResponse) {
  if (result.status !== 'conflicted') {
    await moveProposal(a, result.head)
    return
  }
  a.branchHead = result.stream_head ?? ''
  if (!a.branchHead)
    throw new Error('The server did not identify the branch revision used for this conflict.')
  const prepared = await Promise.all(
    mergeRows(result.conflicts).map((conflict) => prepareRow(a, conflict)),
  )
  if (!current(a)) return
  const paths = new Set(
    prepared.flatMap((row) =>
      row.structural
        ? [
            ...row.structural.paths,
            ...row.structural.mine.map((entry) => entry.path),
            ...row.structural.theirs.map((entry) => entry.path),
          ]
        : [row.conflict.path],
    ),
  )
  if (paths.size > 1000) throw new Error('Too many paths to resolve here. Use the command line.')
  rows.value = prepared
  phase.value = 'ready'
}
async function open() {
  invalidate()
  error.value = null
  rows.value = []
  if (
    !['proposals.update_head', 'workspaces.sync_precondition', 'workspaces.conflict_origins'].every(
      (feature) => auth.capabilities?.features.includes(feature),
    )
  ) {
    error.value = asError(
      new Error(
        'Upgrade the server to resolve conflicts with source paths and revision protection.',
      ),
    )
    phase.value = 'error'
    return
  }
  const a: Attempt = {
    repo: props.repo,
    proposal: props.proposal.id,
    stream: props.proposal.stream,
    head: props.proposal.head,
    branchHead: '',
    workspace: workspaceName(),
    token: auth.token,
    structuralBudget: { remaining: 100 },
    controller: new AbortController(),
    created: false,
    released: false,
  }
  active = a
  phase.value = 'loading'
  try {
    const workspace = await api.workspaces.create(
      a.repo,
      { name: a.workspace, stream: a.stream, from_rev: a.head },
      request(a),
    )
    a.created = true
    if (!current(a)) {
      await release(a)
      return
    }
    if (workspace.head !== a.head)
      throw new Error('The workspace did not open at the inspected revision.')
    const result = await api.workspaces.sync(a.repo, a.workspace, [], request(a))
    if (!current(a)) return
    await prepareMerge(a, result)
  } catch (e) {
    // An ambiguous create may have committed; an explicit collision belongs to somebody else.
    a.created ||= !(e instanceof ApiError && e.code === 'already_exists')
    void release(a)
    if (!current(a)) return
    error.value = asError(e)
    phase.value = 'error'
  }
}
function pick(row: Row, choice: ConflictChoice) {
  row.choice = choice
  if (choice === 'mine' || choice === 'theirs') {
    row.editSource = choice
    row.text = row[choice].text ?? ''
  }
}
const unresolved = computed(() => rows.value.filter((row) => row.blocked || !row.choice).length)
const headChanged = computed(
  () =>
    error.value?.code === 'failed_precondition' &&
    /(?:proposal|branch) head changed/.test(error.value.message),
)
const resolutionText = computed(() =>
  rows.value
    .map(
      (row) =>
        `${row.conflict.path}\n${row.choice === 'delete' ? '(delete file)' : row.editable ? row.text : row.choice || '(not selected)'}`,
    )
    .join('\n\n'),
)
function summary(version: ConflictVersion): string {
  if (!version.entry) return 'Deleted in this revision'
  const entry = version.entry
  return `${entry.file_type === 'symlink' ? 'Symbolic link' : 'Regular file'} · ${formatBytes(entry.size)}${entry.executable ? ' · executable' : ''}`
}
function readyToSave(a: Attempt) {
  return current(a) && phase.value === 'ready' && !unresolved.value && !headChanged.value
}
async function save() {
  const a = active
  if (!a || !readyToSave(a)) return
  phase.value = 'saving'
  error.value = null
  try {
    const resolutions = rows.value.flatMap(rowChanges)
    const result = await api.workspaces.sync(a.repo, a.workspace, resolutions, {
      ...request(a),
      expectedStreamHead: a.branchHead,
    })
    if (!current(a)) return
    if (result.status === 'conflicted')
      throw new Error('Some conflicts remain unresolved. Inspect this change again.')
    await moveProposal(a, result.head)
  } catch (e) {
    if (!current(a)) return
    error.value = asError(e)
    phase.value = 'ready'
  }
}
watch(
  () => props.modelValue,
  (value) => {
    if (value) void open()
    else invalidate()
  },
  { immediate: true },
)
watch(
  [() => props.repo, () => props.proposal.id, () => auth.token, () => auth.identity],
  () => {
    invalidate()
    if (props.modelValue) emit('update:modelValue', false)
  },
  { flush: 'sync' },
)
onBeforeUnmount(invalidate)
</script>

<template>
  <Dialog
    :model-value="modelValue"
    title="Resolve the conflicts"
    description="The branch changed the same files. For each one, choose which version to keep, or write the result."
    size="xl"
    :dismissable="phase !== 'saving'"
    @update:model-value="(v) => emit('update:modelValue', v)"
  >
    <div v-if="phase === 'loading' || phase === 'idle'" class="load" aria-busy="true">
      <Skeleton :height="20" width="40%" /><Skeleton :height="140" radius="8px" />
    </div>
    <Alert
      v-else-if="phase === 'error'"
      tone="bad"
      live="alert"
      title="Could not prepare the merge"
    >
      {{ error?.message }}
    </Alert>
    <div v-else class="rows" data-testid="resolver">
      <section v-for="r in rows" :key="r.conflict.path" class="row" :data-path="r.conflict.path">
        <h3>
          <code class="mono">{{ r.conflict.path }}</code>
          <span class="faint">{{ conflictLabel(r.conflict) }}</span>
        </h3>
        <Alert v-if="r.blocked" tone="warn" title="Not here">{{ r.blocked }}</Alert>
        <template v-else-if="r.structural">
          <p>
            Choose one complete version of this path. Files from the other version will be removed.
          </p>
          <div class="pick" role="group" :aria-label="`Keep for ${r.conflict.path}`">
            <Button
              :variant="r.choice === 'mine' ? 'primary' : 'secondary'"
              :aria-pressed="r.choice === 'mine'"
              @click="r.choice = 'mine'"
              >{{ structuralLabel('mine', r.structural.mine) }}</Button
            >
            <Button
              :variant="r.choice === 'theirs' ? 'primary' : 'secondary'"
              :aria-pressed="r.choice === 'theirs'"
              @click="r.choice = 'theirs'"
              >{{ structuralLabel('the branch’s', r.structural.theirs) }}</Button
            >
          </div>
          <p v-if="r.choice" class="faint">
            This choice removes {{ removedPaths(r).length }} other
            {{ removedPaths(r).length === 1 ? 'path' : 'paths' }} from the merge result.
          </p>
          <details>
            <summary>Files in each version</summary>
            <template v-if="r.choice"
              ><p><strong>Paths removed by this choice</strong></p>
              <ul>
                <li v-for="path in removedPaths(r)" :key="path">
                  <code>{{ path }}</code>
                </li>
              </ul></template
            >
            <p><strong>Your version</strong></p>
            <p v-if="!r.structural.mine.length">No file or folder at this path.</p>
            <ul>
              <li v-for="entry in r.structural.mine" :key="entry.path">
                <code>{{ entry.path }}</code> · {{ entry.file_type
                }}{{ entry.executable ? ' · executable' : '' }}
              </li>
            </ul>
            <p><strong>The branch’s version</strong></p>
            <p v-if="!r.structural.theirs.length">No file or folder at this path.</p>
            <ul>
              <li v-for="entry in r.structural.theirs" :key="entry.path">
                <code>{{ entry.path }}</code> · {{ entry.file_type
                }}{{ entry.executable ? ' · executable' : '' }}
              </li>
            </ul>
          </details>
        </template>
        <template v-else>
          <div class="cols">
            <div>
              <h4>The branch’s version</h4>
              <p
                v-if="r.conflict.origin && r.conflict.origin.theirs !== r.conflict.path"
                class="faint"
              >
                Source: <code>{{ r.conflict.origin.theirs }}</code>
              </p>
              <p class="faint">{{ summary(r.theirs) }}</p>
              <pre
                class="ver"
                tabindex="0"
                role="region"
                :aria-label="`The branch’s version of ${r.conflict.path}`"
                >{{
                  r.theirs.text ??
                  (r.theirs.entry
                    ? 'No text preview. The original file will be preserved.'
                    : '(the branch deleted this file)')
                }}</pre>
            </div>
            <div>
              <h4>Your version</h4>
              <p
                v-if="r.conflict.origin && r.conflict.origin.ours !== r.conflict.path"
                class="faint"
              >
                Source: <code>{{ r.conflict.origin.ours }}</code>
              </p>
              <p class="faint">{{ summary(r.mine) }}</p>
              <pre
                class="ver"
                tabindex="0"
                role="region"
                :aria-label="`Your version of ${r.conflict.path}`"
                >{{
                  r.mine.text ??
                  (r.mine.entry
                    ? 'No text preview. The original file will be preserved.'
                    : '(you deleted this file)')
                }}</pre>
            </div>
          </div>
          <p v-if="!r.editable" class="faint">
            Choose a complete file version. Its bytes, file type and executable mode stay unchanged.
          </p>
          <div class="pick" role="group" :aria-label="`Keep for ${r.conflict.path}`">
            <Button
              size="sm"
              :variant="r.choice === 'theirs' ? 'primary' : 'secondary'"
              :aria-pressed="r.choice === 'theirs'"
              @click="pick(r, 'theirs')"
              >Keep the branch’s</Button
            >
            <Button
              size="sm"
              :variant="r.choice === 'mine' ? 'primary' : 'secondary'"
              :aria-pressed="r.choice === 'mine'"
              @click="pick(r, 'mine')"
              >Keep mine</Button
            >
            <Button
              size="sm"
              :variant="r.choice === 'delete' ? 'primary' : 'secondary'"
              :aria-pressed="r.choice === 'delete'"
              @click="pick(r, 'delete')"
              >Delete the file</Button
            >
          </div>
          <TextArea
            v-if="r.editable && r.choice !== 'delete' && r[r.editSource].entry"
            v-model="r.text"
            :label="`Result for ${r.conflict.path}`"
            :rows="8"
            mono
            @update:model-value="r.choice = 'edit'"
          />
        </template>
      </section>
      <Alert v-if="headChanged" tone="warn" live="alert" title="A revision changed">
        Your resolution has not replaced the newer changes. Copy your work, close this dialog, and
        inspect the updated branch and change request before resolving again.
        <CopyButton :text="resolutionText" label="Copy resolution text" with-label />
      </Alert>
      <Alert v-else-if="error" tone="bad" live="alert" title="Not saved">{{ error.message }}</Alert>
    </div>
    <template #footer>
      <Button :disabled="phase === 'saving'" @click="emit('update:modelValue', false)"
        >Cancel</Button
      >
      <Button
        variant="primary"
        :loading="phase === 'saving'"
        :disabled="phase !== 'ready' || unresolved > 0 || headChanged"
        data-testid="resolve-save"
        @click="save"
        >Save the resolution</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.load,
.rows {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.row {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  padding-bottom: var(--s-4);
  border-bottom: 1px solid var(--border);
}
.row h3 {
  display: flex;
  gap: var(--s-3);
  align-items: baseline;
  flex-wrap: wrap;
  margin: 0;
  font-size: var(--fs-md, 1rem);
}
.row h4 {
  margin: 0 0 var(--s-1);
  font-size: var(--fs-sm);
  color: var(--text-2);
}
.cols {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: var(--s-3);
}
.ver {
  margin: 0;
  padding: var(--s-2) var(--s-3);
  background: var(--code-bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  max-height: 14rem;
  overflow: auto;
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
  white-space: pre;
}
.pick {
  display: flex;
  gap: var(--s-2);
  flex-wrap: wrap;
}
</style>
