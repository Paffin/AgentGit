<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Conflict, FileChange, Proposal } from '@/api/types'
import { canResolveHere, deleteFile, putFile, workspaceName } from '@/lib/edit'
import { conflictLabel } from '@/lib/conflict-label'
import { decodeUtf8, looksBinary } from '@/lib/text'
import { useToasts } from '@/stores/toasts'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextArea from '@/ui/TextArea.vue'
import CopyButton from '@/ui/CopyButton.vue'

/**
 * Resolve the conflicts of your own change request in the browser. The branch has moved under the change; for
 * each file that both sides changed, see both versions and choose or write the result. Nothing is guessed: the
 * server merges everything else, and the change request moves to the merged version only when every conflict has
 * an answer.
 */
const props = defineProps<{ modelValue: boolean; repo: string; proposal: Proposal }>()
const emit = defineEmits<{ 'update:modelValue': [open: boolean]; done: [] }>()
const toasts = useToasts()
const auth = useAuth()

type Choice = 'mine' | 'theirs' | 'delete' | 'edit'
interface Row {
  conflict: Conflict
  mine: string | null
  theirs: string | null
  /** Why it cannot be settled here (binary, renamed, …). */
  blocked: string
  choice: Choice
  text: string
}

const phase = ref<'idle' | 'loading' | 'ready' | 'saving' | 'error'>('idle')
const rows = ref<Row[]>([])
const error = ref<ApiError | null>(null)
let draft = ''
let inspectedHead = ''

const asError = (e: unknown) =>
  e instanceof ApiError
    ? e
    : new ApiError({ status: 0, code: 'internal', message: describeError(e) })

async function read(rev: string, path: string): Promise<string | null | 'binary'> {
  try {
    const f = await api.tree.file(props.repo, rev, path, 512 * 1024)
    if (f.truncated) return 'binary'
    return looksBinary(f.bytes) ? 'binary' : decodeUtf8(f.bytes)
  } catch {
    return null
  }
}

async function open() {
  inspectedHead = props.proposal.head
  if (!auth.capabilities?.features.includes('proposals.update_head')) {
    error.value = new ApiError({
      status: 0,
      code: 'failed_precondition',
      message: 'Upgrade the server to resolve conflicts with concurrent-update protection.',
    })
    phase.value = 'error'
    return
  }
  phase.value = 'loading'
  error.value = null
  rows.value = []
  draft = workspaceName()
  try {
    await api.workspaces.create(props.repo, {
      name: draft,
      stream: props.proposal.stream,
      from_rev: inspectedHead,
    })
    const res = await api.workspaces.sync(props.repo, draft)
    if (res.status !== 'conflicted') {
      // The branch merges cleanly now (it moved again, or someone else resolved it).
      await moveProposal(res.head)
      return
    }
    const theirsRev = (await api.streams.head(props.repo, props.proposal.stream)).head
    rows.value = await Promise.all(
      res.conflicts.map(async (c): Promise<Row> => {
        const [mine, theirs] = await Promise.all([
          read(inspectedHead, c.path),
          read(theirsRev, c.path),
        ])
        const blocked = !canResolveHere(c.kind)
          ? `${conflictLabel(c)}: resolve this one with the command line.`
          : mine === 'binary' || theirs === 'binary'
            ? 'This file is binary or too large to edit here: use the command line.'
            : ''
        const m = typeof mine === 'string' ? mine : null
        const t = typeof theirs === 'string' ? theirs : null
        return {
          conflict: c,
          mine: m,
          theirs: t,
          blocked,
          choice: m !== null ? 'mine' : 'theirs',
          text: m ?? t ?? '',
        }
      }),
    )
    phase.value = 'ready'
  } catch (e) {
    error.value = asError(e)
    phase.value = 'error'
  }
}

function pick(r: Row, choice: Choice) {
  r.choice = choice
  if (choice === 'mine') r.text = r.mine ?? ''
  else if (choice === 'theirs') r.text = r.theirs ?? ''
}

const unresolved = computed(() => rows.value.filter((r) => r.blocked).length)
const headChanged = computed(
  () =>
    error.value?.code === 'failed_precondition' &&
    error.value.message.includes('proposal head changed'),
)
const resolutionText = computed(() =>
  rows.value
    .map((r) => `${r.conflict.path}\n${r.choice === 'delete' ? '(delete file)' : r.text}`)
    .join('\n\n'),
)

async function moveProposal(head: string) {
  await api.proposals.update(props.repo, props.proposal.id, head, inspectedHead)
  toasts.ok('Conflicts resolved', 'The change request now includes the latest branch.')
  emit('update:modelValue', false)
  emit('done')
}

async function save() {
  phase.value = 'saving'
  error.value = null
  try {
    const resolutions: FileChange[] = rows.value.map((r) =>
      r.choice === 'delete' ? deleteFile(r.conflict.path) : putFile(r.conflict.path, r.text),
    )
    const res = await api.workspaces.sync(props.repo, draft, resolutions)
    if (res.status === 'conflicted') {
      error.value = new ApiError({
        status: 409,
        code: 'conflict',
        message: 'Some conflicts are still open. Choose a result for every file.',
      })
      phase.value = 'ready'
      return
    }
    await moveProposal(res.head)
  } catch (e) {
    error.value = asError(e)
    phase.value = 'ready'
  }
}

async function cleanup() {
  const name = draft
  draft = ''
  if (!name) return
  try {
    await api.workspaces.remove(props.repo, name)
  } catch {
    // A leftover draft is harmless.
  }
}

watch(
  () => props.modelValue,
  (on) => {
    if (on) void open()
    else {
      phase.value = 'idle'
      void cleanup()
    }
  },
  { immediate: true },
)
onBeforeUnmount(() => void cleanup())
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
        <template v-else>
          <div class="cols">
            <div>
              <h4>The branch’s version</h4>
              <pre
                class="ver"
                tabindex="0"
                role="region"
                :aria-label="`The branch’s version of ${r.conflict.path}`"
                >{{ r.theirs ?? '(the branch deleted this file)' }}</pre>
            </div>
            <div>
              <h4>Your version</h4>
              <pre
                class="ver"
                tabindex="0"
                role="region"
                :aria-label="`Your version of ${r.conflict.path}`"
                >{{ r.mine ?? '(you deleted this file)' }}</pre>
            </div>
          </div>
          <div class="pick" role="group" :aria-label="`Keep for ${r.conflict.path}`">
            <Button
              size="sm"
              :variant="r.choice === 'theirs' ? 'primary' : 'secondary'"
              @click="pick(r, 'theirs')"
              >Keep the branch’s</Button
            >
            <Button
              size="sm"
              :variant="r.choice === 'mine' ? 'primary' : 'secondary'"
              @click="pick(r, 'mine')"
              >Keep mine</Button
            >
            <Button
              size="sm"
              :variant="r.choice === 'delete' ? 'primary' : 'secondary'"
              @click="pick(r, 'delete')"
              >Delete the file</Button
            >
          </div>
          <TextArea
            v-if="r.choice !== 'delete'"
            v-model="r.text"
            :label="`Result for ${r.conflict.path}`"
            :rows="8"
            mono
            @update:model-value="r.choice = 'edit'"
          />
        </template>
      </section>
      <Alert v-if="headChanged" tone="warn" live="alert" title="The change request changed">
        Your resolution has not replaced the newer changes. Copy your work, close this dialog, and
        inspect the updated change request before resolving again.
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
