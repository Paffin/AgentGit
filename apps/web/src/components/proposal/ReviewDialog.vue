<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Proposal } from '@/api/types'
import { shortId } from '@/lib/format'
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

const decision = ref<Decision>('approve')
const body = ref('')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const reviewedHead = ref(props.proposal.head)
const headChanged = computed(() => reviewedHead.value !== props.proposal.head)

function reset() {
  decision.value = 'approve'
  body.value = ''
  error.value = null
  reviewedHead.value = props.proposal.head
}

// Closing the dialog lets the reviewer inspect the diff without losing their work.
// A different proposal must start a new review. Head changes keep the draft for re-review.
watch([() => props.repo, () => props.proposal.id], reset)

const options = [
  { value: 'approve', label: 'Approve', icon: 'check' },
  { value: 'request_changes', label: 'Request changes', icon: 'alert' },
  { value: 'comment', label: 'Comment', icon: 'comment' },
] as const

async function submit() {
  if (busy.value || headChanged.value) return
  busy.value = true
  error.value = null
  try {
    const capabilities = await api.capabilities()
    if (!capabilities.features.includes('proposals.review_head')) {
      throw new ApiError({
        status: 412,
        code: 'failed_precondition',
        message:
          'This server cannot bind your review to the code you inspected. Update the server before submitting reviews.',
      })
    }
    await api.proposals.review(props.repo, props.proposal.id, {
      decision: decision.value,
      expected_head: reviewedHead.value,
      body: body.value.trim() || undefined,
    })
    toasts.ok(
      decision.value === 'approve'
        ? 'Approval recorded'
        : decision.value === 'request_changes'
          ? 'Changes requested'
          : 'Comment added',
    )
    reset()
    emit('update:modelValue', false)
    emit('done')
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
  <Dialog
    :model-value="modelValue"
    title="Write a review"
    :description="`Your decision is recorded against head ${shortId(proposal.head, 8)}. If the head changes, it stops counting.`"
    :dismissable="!busy"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <form id="review-form" class="form" @submit.prevent="submit">
      <Alert v-if="headChanged" tone="warn" live="alert" title="The proposal changed">
        Your comment is kept. Review the updated changes before submitting your decision.
        <Button size="sm" @click="reviewedHead = proposal.head"
          >I reviewed the updated changes</Button
        >
      </Alert>
      <Segmented v-model="decision" legend="Decision" :options="options" />
      <TextArea
        v-model="body"
        label="Comment"
        :rows="5"
        placeholder="What did you check? Anything the author should know?"
        hint="Plain text. Your unfinished review stays here while you inspect this proposal; reloading the page clears it."
      />
      <Alert v-if="error" tone="bad" live="alert" title="The server did not accept the review">
        {{ error.message }}
        <p v-if="error.requestId" class="faint mono">request {{ error.requestId }}</p>
      </Alert>
    </form>
    <template #footer>
      <Button :disabled="busy" @click="emit('update:modelValue', false)">Cancel</Button>
      <Button
        type="submit"
        form="review-form"
        variant="primary"
        :loading="busy"
        :disabled="headChanged"
        >Submit review</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
</style>
