<script setup lang="ts">
import { ref, watch } from 'vue'
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

watch(
  () => props.modelValue,
  (open) => {
    if (open) {
      decision.value = 'approve'
      body.value = ''
      error.value = null
    }
  },
)

const options = [
  { value: 'approve', label: 'Approve', icon: 'check' },
  { value: 'request_changes', label: 'Request changes', icon: 'alert' },
  { value: 'comment', label: 'Comment', icon: 'comment' },
] as const

async function submit() {
  busy.value = true
  error.value = null
  try {
    await api.proposals.review(props.repo, props.proposal.id, {
      decision: decision.value,
      body: body.value.trim() || undefined,
    })
    toasts.ok(
      decision.value === 'approve'
        ? 'Approval recorded'
        : decision.value === 'request_changes'
          ? 'Changes requested'
          : 'Comment added',
    )
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
      <Segmented v-model="decision" legend="Decision" :options="options" />
      <TextArea
        v-model="body"
        label="Comment"
        :rows="5"
        placeholder="What did you check? Anything the author should know?"
        hint="Plain text."
      />
      <Alert v-if="error" tone="bad" live="alert" title="The server did not accept the review">
        {{ error.message }}
        <p v-if="error.requestId" class="faint mono">request {{ error.requestId }}</p>
      </Alert>
    </form>
    <template #footer>
      <Button :disabled="busy" @click="emit('update:modelValue', false)">Cancel</Button>
      <Button type="submit" form="review-form" variant="primary" :loading="busy"
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
