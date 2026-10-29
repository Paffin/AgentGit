<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { Proposal, Review } from '@/api/types'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { samePrincipal } from '@/lib/identity'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import TextArea from '@/ui/TextArea.vue'

const props = defineProps<{
  repo: string
  proposal: Proposal
  reviewers: string[]
  reviews: readonly Review[]
}>()
const emit = defineEmits<{ saved: [] }>()
const auth = useAuth()
const supported = computed(() => auth.capabilities?.features.includes('proposals.review_requests'))
const allowed = computed(
  () =>
    supported.value &&
    props.proposal.state === 'open' &&
    samePrincipal(props.proposal.author, auth.identity) &&
    auth.can(props.repo, 'editFiles'),
)
const reviewed = (identity: string) =>
  props.reviews.some(
    (review) =>
      samePrincipal(review.reviewer, identity) &&
      review.head === props.proposal.head &&
      (review.decision === 'approve' || review.decision === 'request_changes'),
  )
const open = ref(false),
  busy = ref(false),
  draft = ref(''),
  error = ref('')
const selected = computed(() =>
  [
    ...new Set(
      draft.value
        .split(/[\n,]+/)
        .map((value) => value.trim())
        .filter(Boolean),
    ),
  ].sort(),
)
const invalid = computed(() => (selected.value.length > 20 ? 'Request up to 20 reviewers.' : ''))
let expected: string[] = [],
  generation = 0,
  controller: AbortController | undefined
function reset() {
  generation++
  controller?.abort()
  open.value = busy.value = false
  error.value = ''
}
watch([() => props.repo, () => props.proposal.id, () => auth.token, () => auth.identity], reset, {
  flush: 'sync',
})
onBeforeUnmount(reset)
function edit() {
  expected = [...props.reviewers]
  draft.value = expected.join('\n')
  error.value = ''
  open.value = true
}
async function save() {
  if (!allowed.value || busy.value || invalid.value) return
  const mine = generation
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.proposals.setReviewers(props.repo, props.proposal.id, selected.value, expected, {
      token: auth.token,
      signal: controller.signal,
      quietUnauthenticated: true,
    })
    if (mine !== generation) return
    open.value = false
    emit('saved')
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <section v-if="supported" class="reviewers" aria-label="Requested reviewers">
    <div class="heading">
      <h3>Reviewers</h3>
      <Button v-if="allowed" size="sm" variant="ghost" @click="edit">Request review</Button>
    </div>
    <ul v-if="reviewers.length">
      <li v-for="reviewer in reviewers" :key="reviewer">
        <IdentityLabel :identity="reviewer" :size="22" />
        <span class="status">{{
          reviewed(reviewer) ? 'Reviewed this revision' : 'Review requested'
        }}</span>
      </li>
    </ul>
    <p v-else class="muted">No reviewers requested.</p>
  </section>
  <Dialog v-model="open" title="Request reviewers" :dismissable="!busy" size="sm">
    <form id="request-reviewers" @submit.prevent="save">
      <TextArea
        v-model="draft"
        label="Reviewer identities"
        :rows="5"
        :disabled="busy"
        hint="One identity per line, such as user:alex. Requests do not grant access or count as approval."
        :error="invalid"
      />
      <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
    </form>
    <template #footer>
      <Button :disabled="busy" @click="open = false">Cancel</Button>
      <Button variant="primary" :disabled="!!invalid || !allowed" :loading="busy" @click="save"
        >Save reviewer requests</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
/* Hallmark · shape: app screen · theme: locked AgentGit · scope: component · v2.0.0 */
/* Critique: reviewers are a quiet list in the review rail; only the author sees its editing action. */
.reviewers {
  min-width: 0;
}
.heading {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
}
h3 {
  font-size: var(--fs-md);
  font-weight: 600;
}
ul {
  list-style: none;
  margin: var(--s-3) 0 0;
  padding: 0;
  display: grid;
  gap: var(--s-3);
}
li {
  display: grid;
  gap: var(--s-1);
}
.status,
.muted {
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.muted {
  margin-top: var(--s-2);
}
form {
  display: grid;
  gap: var(--s-3);
}
</style>
