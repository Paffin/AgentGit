<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { formatLineQuote, type LineQuote } from '@/lib/diffDiscussion'
import type { Proposal } from '@/api/types'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import { useCommentDraft } from '@/composables/useCommentDraft'
import { useAccountCommentDraft } from '@/composables/useAccountCommentDraft'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import TextArea from '@/ui/TextArea.vue'

const props = defineProps<{
  repo: string
  proposal: Proposal
  quote?: LineQuote | null
  quoteText?: string
}>()
const emit = defineEmits<{ done: []; quoted: [] }>()
const auth = useAuth(),
  toasts = useToasts()
const supported = computed(() => !!auth.capabilities?.features.includes('proposals.comments'))
const allowed = computed(() => auth.can(props.repo, 'commentProposal'))
const draft = useCommentDraft(
  () => (auth.identity ? JSON.stringify([auth.identity, props.repo, props.proposal.id]) : ''),
  () => props.proposal.head,
)
const account = useAccountCommentDraft({
  repo: () => props.repo,
  proposal: () => props.proposal.id,
  open: () => supported.value && allowed.value,
  local: draft,
})
const form = ref<HTMLFormElement | null>(null)
watch(
  () => props.quote,
  async (quote) => {
    if (!quote || !supported.value || !allowed.value) return
    if (!draft.body.value) draft.head.value = quote.head
    const existing = draft.body.value.trimEnd()
    draft.body.value =
      [existing, props.quoteText?.trimEnd(), formatLineQuote(quote)].filter(Boolean).join('\n\n') +
      '\n\n'
    emit('quoted')
    await nextTick()
    form.value?.querySelector('textarea')?.focus()
  },
  { immediate: true },
)
const stale = computed(() => draft.head.value !== props.proposal.head)
const busy = ref(false),
  error = ref('')
let generation = 0,
  controller: AbortController | undefined
watch(
  [() => auth.token, () => props.repo, () => props.proposal.id],
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
async function saveToAccount() {
  if (busy.value) return
  if (await account.save()) toasts.ok('Draft saved to your account')
}
async function submit() {
  if (
    busy.value ||
    account.loading.value ||
    account.saving.value ||
    !supported.value ||
    !allowed.value ||
    stale.value ||
    !draft.body.value.trim()
  )
    return
  const mine = generation
  const cleanupVersion = account.cleanupVersion()
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.proposals.comment(
      props.repo,
      props.proposal.id,
      {
        body: draft.body.value,
        expected_head: draft.head.value,
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
    toasts.ok('Comment added')
    const cleanupError = await account.clearAfterSubmit(cleanupVersion)
    if (mine !== generation) return
    if (cleanupError)
      toasts.warn(`Comment added, but the account draft could not be cleared: ${cleanupError}`)
    emit('done')
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <form v-if="supported && allowed" ref="form" class="comment-form" @submit.prevent="submit">
    <TextArea
      v-model="draft.body.value"
      label="Add to the discussion"
      :rows="4"
      :disabled="busy || account.loading.value"
      :hint="
        account.supported.value
          ? 'Comments do not approve changes. Drafts stay in this tab until you save them to your account.'
          : 'Comments do not approve changes. Your draft is saved in this tab.'
      "
    />
    <Alert v-if="account.available.value" tone="info" title="An account draft is available">
      Keep your tab draft or restore the saved account draft.
      <div class="draft-actions">
        <Button size="sm" :disabled="busy" @click="account.useAccount()"
          >Restore account draft</Button
        >
        <Button size="sm" :disabled="busy" @click="account.keepLocal()">Keep tab draft</Button>
      </div>
    </Alert>
    <Alert v-else-if="account.restored.value" tone="info">Your account draft was restored.</Alert>
    <Alert v-if="account.error.value" tone="warn">
      {{ account.error.value }}
      <Button
        size="sm"
        :disabled="busy || account.loading.value || account.saving.value"
        @click="account.load()"
        >Reload account draft</Button
      >
    </Alert>
    <Alert v-if="draft.error.value" tone="warn">{{ draft.error.value }}</Alert>
    <Alert v-if="stale" tone="warn" title="The change request has a new head">
      Your draft refers to an earlier head. Inspect the current changes before posting.
      <Button size="sm" :disabled="busy" @click="draft.head.value = proposal.head"
        >I have inspected the current changes</Button
      >
    </Alert>
    <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
    <div class="draft-actions">
      <Button
        type="submit"
        variant="primary"
        :loading="busy"
        :disabled="
          stale || !draft.body.value.trim() || account.loading.value || account.saving.value
        "
        >Add comment</Button
      >
      <Button
        v-if="account.supported.value"
        :loading="account.saving.value"
        :disabled="busy || !account.canSave.value"
        @click="saveToAccount"
        >Save draft</Button
      >
    </div>
  </form>
</template>

<style scoped>
.comment-form {
  display: flex;
  flex-direction: column;
  align-items: start;
  gap: var(--s-3);
}
.draft-actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.comment-form :deep(.field) {
  width: 100%;
}
</style>
