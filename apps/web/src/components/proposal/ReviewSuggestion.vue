<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { Proposal, ReviewThread } from '@/api/types'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { samePrincipal } from '@/lib/identity'
import type { ReviewSuggestion } from '@/lib/reviewSuggestion'
import { applySuggestion, previewSuggestion } from '@/lib/applySuggestion'
import type { SuggestionPreview } from '@/lib/applySuggestion'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import CopyButton from '@/ui/CopyButton.vue'
import Dialog from '@/ui/Dialog.vue'

const props = defineProps<{
  repo: string
  proposal: Proposal
  thread: ReviewThread
  suggestion: ReviewSuggestion
}>()
const emit = defineEmits<{ applied: [] }>()
const auth = useAuth()
const current = computed(() => props.thread.anchor?.head === props.proposal.head)
const blocked = computed(() => {
  if (!current.value)
    return 'This suggestion belongs to an earlier revision. Inspect the current code before reusing it.'
  if (props.thread.anchor?.side !== 'new')
    return 'Only suggestions on the proposed version can be applied.'
  if (props.thread.resolved) return 'Reopen this discussion before applying the suggestion.'
  if (props.proposal.state !== 'open') return 'Only open change requests can be changed.'
  if (!samePrincipal(props.proposal.author, auth.identity) || !auth.can(props.repo, 'editFiles'))
    return 'The change request’s author needs write access to apply this suggestion.'
  if (!auth.capabilities?.features.includes('proposals.update_head'))
    return 'Upgrade the server to apply suggestions with concurrent-update protection.'
  return ''
})
const canApply = computed(() => !blocked.value)
const open = ref(false),
  busy = ref(false),
  error = ref('')
const preview = ref<SuggestionPreview | null>(null)
let generation = 0,
  controller: AbortController | undefined
function reset() {
  generation++
  controller?.abort()
  open.value = false
  busy.value = false
  error.value = ''
  preview.value = null
}
watch(
  [
    () => props.repo,
    () => props.proposal.id,
    () => props.proposal.head,
    () => props.thread.id,
    () => auth.token,
  ],
  reset,
  { flush: 'sync' },
)
onBeforeUnmount(reset)
async function inspect() {
  if (!canApply.value || !props.thread.anchor || busy.value) return
  const mine = generation
  controller = new AbortController()
  open.value = true
  busy.value = true
  error.value = ''
  try {
    const prepared = await previewSuggestion(
      api,
      props.repo,
      props.thread.anchor,
      props.suggestion.replacement,
      { token: auth.token, quietUnauthenticated: true, signal: controller.signal },
    )
    if (mine === generation) preview.value = prepared
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
async function apply() {
  if (!canApply.value || !preview.value || busy.value) return
  const mine = generation
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await applySuggestion(
      api,
      props.repo,
      props.proposal.id,
      props.proposal.stream,
      props.proposal.head,
      preview.value,
      { token: auth.token, quietUnauthenticated: true, signal: controller.signal },
    )
    if (mine !== generation) return
    open.value = false
    preview.value = null
    emit('applied')
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <section class="suggestion" aria-label="Suggested replacement">
    <strong>Suggested replacement</strong>
    <pre v-if="suggestion.replacement" class="replacement">{{ suggestion.replacement }}</pre>
    <p v-else>Delete this line.</p>
    <div class="actions">
      <CopyButton :text="suggestion.replacement" label="Copy suggested code" with-label />
      <Button v-if="canApply" size="sm" @click="inspect">Review and apply suggestion</Button>
    </div>
    <p v-if="blocked" class="muted">{{ blocked }}</p>
    <p v-else class="muted">
      Applying creates a new revision. Checks and approvals must match that revision.
    </p>
  </section>
  <Dialog v-model="open" title="Apply this suggestion?" :dismissable="!busy" size="lg">
    <p>{{ thread.anchor?.path }} · line {{ thread.anchor?.line }}</p>
    <p class="muted">
      Replaces only this line in the inspected revision. The discussion stays open for review.
    </p>
    <h3>Current line</h3>
    <pre class="replacement">{{ thread.anchor?.text }}</pre>
    <h3>Replacement</h3>
    <pre v-if="suggestion.replacement" class="replacement">{{ suggestion.replacement }}</pre>
    <p v-else>Delete this line.</p>
    <p v-if="busy" role="status">Preparing the change…</p>
    <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
    <template #footer>
      <Button :disabled="busy" @click="open = false">Cancel</Button>
      <Button variant="primary" :disabled="!preview || !canApply" :loading="busy" @click="apply"
        >Apply suggestion</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.suggestion {
  display: grid;
  gap: var(--s-2);
  padding: var(--s-3);
  background: var(--surface-2);
  border-left: 3px solid var(--border);
  min-width: 0;
}
.replacement {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  font-family: var(--font-mono);
}
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.muted {
  color: var(--text-2);
}
</style>
