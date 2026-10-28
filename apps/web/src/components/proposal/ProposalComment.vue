<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { Proposal } from '@/api/types'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import { useCommentDraft } from '@/composables/useCommentDraft'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import TextArea from '@/ui/TextArea.vue'

const props = defineProps<{ repo: string; proposal: Proposal }>()
const emit = defineEmits<{ done: [] }>()
const auth = useAuth(),
  toasts = useToasts()
const supported = computed(() => !!auth.capabilities?.features.includes('proposals.comments'))
const allowed = computed(() => auth.can(props.repo, 'commentProposal'))
const draft = useCommentDraft(
  () => (auth.identity ? JSON.stringify([auth.identity, props.repo, props.proposal.id]) : ''),
  () => props.proposal.head,
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
async function submit() {
  if (busy.value || !supported.value || !allowed.value || stale.value || !draft.body.value.trim())
    return
  const mine = generation
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
    emit('done')
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <form v-if="supported && allowed" class="comment-form" @submit.prevent="submit">
    <TextArea
      v-model="draft.body.value"
      label="Add to the discussion"
      :rows="4"
      :disabled="busy"
      hint="Comments do not approve changes. Your draft is saved in this tab."
    />
    <Alert v-if="draft.error.value" tone="warn">{{ draft.error.value }}</Alert>
    <Alert v-if="stale" tone="warn" title="The change request has a new head">
      Your draft refers to an earlier head. Inspect the current changes before posting.
      <Button size="sm" :disabled="busy" @click="draft.head.value = proposal.head"
        >I have inspected the current changes</Button
      >
    </Alert>
    <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
    <Button
      type="submit"
      variant="primary"
      :loading="busy"
      :disabled="stale || !draft.body.value.trim()"
      >Add comment</Button
    >
  </form>
</template>

<style scoped>
.comment-form {
  display: flex;
  flex-direction: column;
  align-items: start;
  gap: var(--s-3);
}
.comment-form :deep(.field) {
  width: 100%;
}
</style>
