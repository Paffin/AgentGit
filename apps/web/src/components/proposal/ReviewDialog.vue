<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Proposal } from '@/api/types'
import { shortId } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import Segmented from '@/ui/Segmented.vue'
import TextArea from '@/ui/TextArea.vue'

type Decision = 'approve' | 'request_changes' | 'comment'

const props = defineProps<{ modelValue: boolean; repo: string; proposal: Proposal }>()
const emit = defineEmits<{ 'update:modelValue': [open: boolean]; done: [] }>()
const toasts = useToasts()
const auth = useAuth()

const decision = ref<Decision>('approve')
const body = ref('')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const reviewedHead = ref(props.proposal.head)
const headChanged = computed(() => reviewedHead.value !== props.proposal.head)
const storageError = ref('')
const restored = ref(false)
const needsSignIn = ref(false)
const draftKey = computed(() =>
  auth.identity
    ? 'agentgit.review-draft.v1:' + JSON.stringify([auth.identity, props.repo, props.proposal.id])
    : '',
)
const signInHref = computed(
  () =>
    '/signin?next=' +
    encodeURIComponent(
      '/repos/' +
        props.repo.split('/').map(encodeURIComponent).join('/') +
        '/proposals/' +
        encodeURIComponent(props.proposal.id),
    ),
)
let restoring = false
let blockedStorage = false
let controller: AbortController | undefined
let context = 0

function reset() {
  decision.value = 'approve'
  body.value = ''
  error.value = null
  reviewedHead.value = props.proposal.head
}

function loadDraft() {
  context++
  controller?.abort()
  busy.value = false
  restoring = true
  reset()
  restored.value = false
  storageError.value = ''
  blockedStorage = false
  needsSignIn.value = false
  try {
    const raw = draftKey.value ? sessionStorage.getItem(draftKey.value) : null
    if (raw) {
      if (raw.length > 1024 * 1024) throw new Error('Saved review is too large')
      const saved = JSON.parse(raw)
      if (
        saved.version !== 1 ||
        typeof saved.body !== 'string' ||
        typeof saved.head !== 'string' ||
        !['approve', 'request_changes', 'comment'].includes(saved.decision)
      )
        throw new Error('Invalid saved review')
      body.value = saved.body
      decision.value = saved.decision
      reviewedHead.value = saved.head
      restored.value = true
    }
  } catch {
    blockedStorage = true
    storageError.value =
      'The saved review could not be read and was left untouched. Copy any new comment before leaving.'
  } finally {
    restoring = false
  }
}
function saveDraft() {
  if (restoring || !draftKey.value || blockedStorage) return
  try {
    if (!body.value && decision.value === 'approve') {
      sessionStorage.removeItem(draftKey.value)
    } else {
      sessionStorage.setItem(
        draftKey.value,
        JSON.stringify({
          version: 1,
          body: body.value,
          decision: decision.value,
          head: reviewedHead.value,
        }),
      )
    }
    storageError.value = ''
  } catch {
    storageError.value = 'This tab could not save your review. Copy the comment before leaving.'
  }
}
watch(draftKey, loadDraft, { immediate: true, flush: 'sync' })
watch([body, decision, reviewedHead], saveDraft, { flush: 'sync' })
watch(
  () => [auth.token, auth.status],
  () => {
    context++
    controller?.abort()
    busy.value = false
    needsSignIn.value = false
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
})

const options = [
  { value: 'approve', label: 'Approve', icon: 'check' },
  { value: 'request_changes', label: 'Request changes', icon: 'alert' },
  { value: 'comment', label: 'Comment', icon: 'comment' },
] as const

const decisionMessages: Record<Decision, string> = {
  approve: 'Approval recorded',
  request_changes: 'Changes requested',
  comment: 'Comment added',
}

async function requireAtomicReview() {
  const capabilities = await api.capabilities()
  if (!capabilities.features.includes('proposals.review_head')) {
    throw new ApiError({
      status: 412,
      code: 'failed_precondition',
      message:
        'This server cannot bind your review to the code you inspected. Update the server before submitting reviews.',
    })
  }
}

function reviewError(value: unknown): ApiError {
  return value instanceof ApiError
    ? value
    : new ApiError({ status: 0, code: 'internal', message: describeError(value) })
}

async function submit() {
  if (busy.value || headChanged.value || needsSignIn.value || auth.status !== 'ready') return
  const mine = context
  const token = auth.token
  const current = new AbortController()
  controller = current
  const repository = props.repo
  const id = props.proposal.id
  const submitted = {
    decision: decision.value,
    expected_head: reviewedHead.value,
    body: body.value.trim() || undefined,
  }
  busy.value = true
  error.value = null
  try {
    await requireAtomicReview()
    if (mine !== context) return
    await api.proposals.review(repository, id, submitted, {
      token,
      signal: current.signal,
      quietUnauthenticated: true,
    })
    if (mine !== context) return
    toasts.ok(decisionMessages[submitted.decision])
    restoring = true
    reset()
    restored.value = false
    try {
      if (draftKey.value) sessionStorage.removeItem(draftKey.value)
      blockedStorage = false
      storageError.value = ''
    } catch {
      storageError.value =
        'Review submitted, but the saved draft could not be removed from this tab.'
    }
    restoring = false
    emit('update:modelValue', false)
    emit('done')
  } catch (e) {
    if (mine !== context) return
    error.value = reviewError(e)
    needsSignIn.value = error.value.isUnauthenticated
  } finally {
    if (mine === context) busy.value = false
  }
}
</script>

<template>
  <Dialog
    :model-value="modelValue"
    title="Write a review"
    :description="`Your decision is recorded against head ${shortId(proposal.head, 8)}. If the head changes, it stops counting.`"
    :dismissable="!busy"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <form id="review-form" class="form" @submit.prevent="submit">
      <fieldset :disabled="busy" class="fields">
        <legend class="sr-only">Review details</legend>
        <Alert v-if="headChanged" tone="warn" live="alert" title="The proposal changed">
          Your comment is kept. Review the updated changes before submitting your decision.
          <Button size="sm" @click="reviewedHead = proposal.head"
            >I reviewed the updated changes</Button
          >
        </Alert>
        <p v-if="restored" class="faint" role="status">Your saved review was restored.</p>
        <Alert v-if="storageError" tone="warn" title="Review draft storage">{{
          storageError
        }}</Alert>
        <Segmented v-model="decision" legend="Decision" :options="options" />
        <TextArea
          v-model="body"
          label="Comment"
          :rows="5"
          placeholder="What did you check? Anything the author should know?"
          hint="Plain text. Your unfinished review is saved for this account in this browser tab, including across page reloads."
        />
      </fieldset>
      <Alert v-if="error" tone="bad" live="alert" title="The server did not accept the review">
        {{ error.message }}
        <a v-if="needsSignIn" :href="signInHref">Sign in again to submit your saved review</a>
        <p v-if="error.requestId" class="faint mono">request {{ error.requestId }}</p>
      </Alert>
    </form>
    <template #footer>
      <Button :disabled="busy" @click="emit('update:modelValue', false)"
        >Keep draft and close</Button
      >
      <Button
        type="submit"
        form="review-form"
        variant="primary"
        :loading="busy"
        :disabled="headChanged || needsSignIn || auth.status !== 'ready'"
        >Submit review</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.form,
.fields {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.fields {
  border: 0;
  margin: 0;
  padding: 0;
  min-width: 0;
}
</style>
