<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import type { Proposal } from '@/api/types'
import { formatSuggestion, parseSuggestion } from '@/lib/reviewSuggestion'
import type { LineQuote } from '@/lib/diffDiscussion'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useCommentDraft } from '@/composables/useCommentDraft'
import { useThreadAnchor } from '@/composables/useThreadAnchor'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import TextArea from '@/ui/TextArea.vue'
import CopyButton from '@/ui/CopyButton.vue'

const props = defineProps<{ repo: string; proposal: Proposal; quote?: LineQuote | null }>()
const emit = defineEmits<{ created: []; quoted: []; general: [quote: LineQuote, body: string] }>()
const auth = useAuth()
const scope = computed(() => JSON.stringify([auth.identity, props.repo, props.proposal.id]))
const selected = useThreadAnchor(() => scope.value)
const draft = useCommentDraft(
  () => (selected.anchor.value ? JSON.stringify([scope.value, selected.anchor.value]) : ''),
  () => props.proposal.head,
)
const allowed = computed(() => auth.can(props.repo, 'commentProposal'))
const suggestion = computed(() => parseSuggestion(draft.body.value))
const explanation = computed({
  get: () => suggestion.value?.explanation || '',
  set: (value) => {
    draft.body.value = formatSuggestion(value, suggestion.value?.replacement || '')
  },
})
const replacement = computed({
  get: () => suggestion.value?.replacement || '',
  set: (value) => {
    draft.body.value = formatSuggestion(suggestion.value?.explanation || '', value)
  },
})
function suggest() {
  if (selected.anchor.value?.side === 'new')
    draft.body.value = formatSuggestion(draft.body.value, selected.anchor.value.text)
}
function removeSuggestion() {
  if (suggestion.value)
    draft.body.value = [suggestion.value.explanation, suggestion.value.replacement]
      .filter(Boolean)
      .join('\n\n')
}
const stale = computed(() => selected.anchor.value?.head !== props.proposal.head)
const form = ref<HTMLElement | null>(null)
const busy = ref(false),
  error = ref('')
let generation = 0,
  controller: AbortController | undefined
watch(
  () => props.quote,
  async (quote) => {
    if (!quote || !allowed.value) return
    selected.anchor.value = quote
    draft.head.value = quote.head
    emit('quoted')
    await nextTick()
    form.value?.querySelector('textarea')?.focus()
  },
  { immediate: true },
)
watch(
  [scope, () => auth.token],
  () => {
    generation++
    controller?.abort()
    busy.value = false
    error.value = ''
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  generation++
  controller?.abort()
})
function general() {
  if (!selected.anchor.value) return
  emit('general', selected.anchor.value, draft.body.value)
  selected.anchor.value = null
}
async function create() {
  if (
    !allowed.value ||
    busy.value ||
    stale.value ||
    !selected.anchor.value ||
    !draft.body.value.trim()
  )
    return
  const mine = generation
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.proposals.createThread(
      props.repo,
      props.proposal.id,
      {
        body: draft.body.value,
        expected_head: selected.anchor.value.head,
        anchor: { ...selected.anchor.value, text: '' },
      },
      {
        token: auth.token,
        signal: controller.signal,
        idempotencyKey: draft.requestId.value,
        quietUnauthenticated: true,
      },
    )
    if (mine !== generation) return
    draft.clear()
    selected.anchor.value = null
    emit('created')
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <Alert v-if="selected.error.value" tone="warn">{{ selected.error.value }}</Alert>
  <form
    v-if="selected.anchor.value && allowed"
    ref="form"
    class="thread-composer"
    @submit.prevent="create"
  >
    <h3>Start a line discussion</h3>
    <p>
      <strong>{{ selected.anchor.value.path }}</strong> · {{ selected.anchor.value.side }} line
      {{ selected.anchor.value.line }}
    </p>
    <pre class="source">{{ selected.anchor.value.text }}</pre>
    <p class="muted">The server verifies this line against the exact revision you inspected.</p>
    <template v-if="suggestion">
      <TextArea v-model="explanation" label="Explain this suggestion" :disabled="busy" :rows="2" />
      <TextArea
        v-model="replacement"
        label="Replacement code"
        :disabled="busy"
        :rows="5"
        mono
        hint="Replaces this one line. Multiple replacement lines are allowed. Empty deletes the line. Saved in this tab."
      />
      <Button size="sm" :disabled="busy" @click="removeSuggestion"
        >Keep as ordinary discussion</Button
      >
    </template>
    <TextArea
      v-else
      v-model="draft.body.value"
      label="Start this discussion"
      :rows="4"
      :disabled="busy"
      hint="Your line selection and draft are saved in this tab."
    />
    <Button
      v-if="!suggestion && selected.anchor.value.side === 'new'"
      size="sm"
      :disabled="busy || stale"
      @click="suggest"
      >Suggest a replacement</Button
    >
    <Alert v-if="stale" tone="warn" title="The proposal has changed"
      >Select a line from the current Files changed view before opening this thread. Your text
      remains saved.<CopyButton :text="draft.body.value" label="Copy discussion draft" with-label
    /></Alert>
    <Alert v-if="draft.error.value" tone="warn">{{ draft.error.value }}</Alert>
    <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
    <div class="actions">
      <Button
        type="submit"
        variant="primary"
        :disabled="stale || !draft.body.value.trim()"
        :loading="busy"
        >Start discussion</Button
      >
      <Button :disabled="busy" @click="selected.anchor.value = null">Cancel</Button>
      <Button :disabled="busy" @click="general">Use general discussion instead</Button>
    </div>
  </form>
</template>

<style scoped>
.thread-composer {
  display: grid;
  gap: var(--s-3);
  padding: var(--s-4);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
}
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.source {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  background: var(--surface-2);
  padding: var(--s-3);
}
.muted {
  color: var(--text-2);
}
</style>
