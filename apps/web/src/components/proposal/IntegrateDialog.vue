<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { IntegrationOutcome, Lease, Preparation, Proposal } from '@/api/types'
import BlockerList from '@/components/BlockerList.vue'
import { shortId, plural } from '@/lib/format'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import TextField from '@/ui/TextField.vue'
import { conflictLabel } from '@/lib/conflict-label'

const props = defineProps<{
  modelValue: boolean
  repo: string
  proposal: Proposal
  prep?: Preparation | null
  authorLeases: readonly Lease[]
  /** Why the author's leases are unknown, when they could not be read. */
  leasesError?: string
}>()
const emit = defineEmits<{ 'update:modelValue': [open: boolean]; done: [] }>()
const toasts = useToasts()

const tokens = ref('')
const busy = ref(false)
const outcome = ref<IntegrationOutcome | null>(null)
const error = ref<ApiError | null>(null)

const leaseBlocked = computed(() => (props.prep?.blockers ?? []).some((b) => b.code === 'lease'))

let context = 0
watch(
  [() => props.modelValue, () => props.repo, () => props.proposal.id],
  ([open]) => {
    context++
    busy.value = false
    if (!open) return
    outcome.value = null
    error.value = null
    // Offer the author's exclusive-lease tokens when a lease is in the way; the integrator can edit them.
    tokens.value = leaseBlocked.value
      ? props.authorLeases.map((l) => l.fencing_token).join(', ')
      : ''
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  context++
})

function parseTokens(text: string): number[] | null {
  const parts = text.split(/[\s,]+/).filter(Boolean)
  const nums = parts.map((p) => (/^\d+$/.test(p) ? Number(p) : NaN))
  return nums.some((n) => Number.isNaN(n)) ? null : nums
}
const tokenError = computed(() =>
  parseTokens(tokens.value) === null
    ? 'Fencing tokens are whole numbers, separated by commas.'
    : '',
)

function showOutcome(value: IntegrationOutcome | null, stream: string) {
  if (!value)
    throw new ApiError({
      status: 200,
      code: 'bad_response',
      message: 'The server answered without an outcome.',
    })
  outcome.value = value
  if (value.status === 'integrated')
    toasts.ok('Landed', `${stream} is now at ${shortId(value.new_head, 10)}`)
  // A blocked attempt is also a recorded operation; refresh the page behind the dialog.
  emit('done')
}

async function submit() {
  if (busy.value || !props.modelValue) return
  const mine = context
  const { repo, proposal } = props
  const stream = proposal.stream
  const id = proposal.id
  const parsed = parseTokens(tokens.value)
  if (parsed === null) return
  busy.value = true
  error.value = null
  outcome.value = null
  try {
    const res = await api.integration.integrate(repo, id, parsed)
    if (mine !== context) return
    showOutcome(res.outcome, stream)
  } catch (e) {
    if (mine !== context) return
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    if (mine === context) busy.value = false
  }
}

const status = computed(() => outcome.value?.status ?? '')
const finished = computed(() => status.value === 'integrated')
</script>

<template>
  <Dialog
    :model-value="modelValue"
    title="Land this change request"
    size="lg"
    :dismissable="!busy"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="body">
      <p v-if="prep && prep.strategy" class="what">
        This will move <strong class="mono">{{ proposal.stream }}</strong> by
        <strong>{{
          prep.strategy === 'fast_forward'
            ? 'fast-forward'
            : prep.strategy === 'merge'
              ? 'a deterministic merge'
              : prep.strategy
        }}</strong
        >, changing {{ plural(prep.changed_files.length, 'file') }}. Policy is checked again
        atomically with the update, so the result below is final.
      </p>
      <BlockerList
        v-if="prep && prep.blockers.length && !outcome"
        :blockers="prep.blockers"
        hints
      />

      <TextField
        v-if="!finished"
        v-model="tokens"
        :disabled="busy"
        label="Reservation keys (optional)"
        mono
        placeholder="e.g. 7, 9"
        :error="tokenError"
        hint="Needed when an exclusive lease covers files this change touches: present the current token of the author’s lease."
      />
      <Alert
        v-if="!finished && leasesError"
        tone="warn"
        title="The author’s reservations could not be read"
      >
        {{ leasesError }}. If an exclusive lease covers files this change touches, enter its fencing
        token yourself.
      </Alert>
      <p v-if="!finished && authorLeases.length" class="faint leases">
        Author’s exclusive leases:
        <span v-for="l in authorLeases" :key="l.id" class="mono"
          >{{ l.scope }} (token {{ l.fencing_token }})</span
        >
      </p>

      <Alert v-if="error" tone="bad" live="alert" title="Landing failed">
        {{ error.message }}
        <p v-if="error.requestId" class="faint mono">request {{ error.requestId }}</p>
      </Alert>

      <section v-if="outcome" class="result" :data-status="outcome.status" aria-live="polite">
        <Alert v-if="outcome.status === 'integrated'" tone="ok" title="Landed">
          <p>
            <strong class="mono">{{ proposal.stream }}</strong> is now at
            <code class="mono">{{ shortId(outcome.new_head, 12) }}</code> (version
            {{ outcome.version }}, {{ outcome.strategy.replace('_', '-') }},
            {{ plural(outcome.changed_files, 'file') }} changed).
          </p>
        </Alert>
        <Alert
          v-else-if="outcome.status === 'blocked'"
          tone="warn"
          title="Blocked: the branch was not changed"
        >
          <p>Provide what is missing and run it again; the same operation resumes.</p>
          <BlockerList :blockers="outcome.blockers" hints />
        </Alert>
        <Alert
          v-else-if="outcome.status === 'conflict'"
          tone="bad"
          title="Conflict: the branch was not changed"
        >
          <p>
            The change and the stream edited the same places. Resolve these paths in the author’s
            workspace and update the proposal.
          </p>
          <ul class="conf">
            <li v-for="c in outcome.conflicts" :key="c.path" class="mono">
              {{ c.path }} <span class="faint">({{ conflictLabel(c) }})</span>
            </li>
          </ul>
        </Alert>
        <Alert
          v-else-if="outcome.status === 'denied'"
          tone="bad"
          title="Refused by the rules: the branch was not changed"
        >
          <BlockerList :blockers="outcome.blockers" hints />
        </Alert>
        <Alert v-else tone="bad" :title="`Not possible (${outcome.status})`">
          {{ outcome.reason || 'The server reported the request as invalid.' }}
        </Alert>
      </section>
    </div>
    <template #footer>
      <Button :disabled="busy" @click="emit('update:modelValue', false)">{{
        finished ? 'Close' : 'Cancel'
      }}</Button>
      <Button
        v-if="!finished"
        variant="primary"
        icon="merge"
        :loading="busy"
        :disabled="!!tokenError"
        @click="submit"
      >
        {{ outcome ? 'Try again' : 'Land' }}
      </Button>
    </template>
  </Dialog>
</template>

<style scoped>
.body {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.what {
  color: var(--text-2);
}
.leases {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 12px;
  font-size: var(--fs-sm);
}
.result :deep(ul) {
  margin-top: var(--s-2);
}
.conf {
  list-style: none;
  margin: var(--s-2) 0 0;
  padding: 0;
}
</style>
