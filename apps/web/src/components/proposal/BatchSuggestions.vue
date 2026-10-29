<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { Proposal, ReviewThread } from '@/api/types'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { samePrincipal } from '@/lib/identity'
import { parseSuggestion } from '@/lib/reviewSuggestion'
import { applySuggestions, previewSuggestions } from '@/lib/applySuggestion'
import type { SuggestionPreview } from '@/lib/applySuggestion'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Checkbox from '@/ui/Checkbox.vue'
import Dialog from '@/ui/Dialog.vue'

const props = defineProps<{ repo: string; proposal: Proposal; threads: ReviewThread[] }>()
const emit = defineEmits<{ applied: [] }>()
const auth = useAuth()
const allowed = computed(
  () =>
    props.proposal.state === 'open' &&
    samePrincipal(props.proposal.author, auth.identity) &&
    auth.can(props.repo, 'editFiles') &&
    auth.capabilities?.features.includes('proposals.update_head'),
)
const candidates = computed(() =>
  props.threads.flatMap((thread) => {
    const suggestion = parseSuggestion(thread.body)
    if (
      !suggestion ||
      thread.resolved ||
      !thread.anchor ||
      thread.anchor.side !== 'new' ||
      thread.anchor.head !== props.proposal.head
    )
      return []
    return [
      {
        id: thread.id,
        version: thread.version,
        anchor: thread.anchor,
        replacement: suggestion.replacement,
        lines: suggestion.lines,
      },
    ]
  }),
)
const selected = ref<string[]>([])
const choices = computed(() => candidates.value.filter((item) => selected.value.includes(item.id)))
const open = ref(false),
  busy = ref(false),
  error = ref(''),
  failed = ref(false)
const previews = ref<SuggestionPreview[]>([])
let generation = 0,
  controller: AbortController | undefined,
  preparedHead = ''
function resetPreview() {
  generation++
  controller?.abort()
  open.value = busy.value = failed.value = false
  error.value = ''
  previews.value = []
}
function reset() {
  selected.value = []
  resetPreview()
}
watch(
  [
    () => props.repo,
    () => props.proposal.id,
    () => props.proposal.head,
    () => auth.token,
    () => auth.identity,
  ],
  reset,
  { flush: 'sync' },
)
watch(() => JSON.stringify(choices.value), resetPreview, { flush: 'sync' })
onBeforeUnmount(resetPreview)
function choose(id: string, checked: boolean) {
  selected.value = checked
    ? [...selected.value, id]
    : selected.value.filter((value) => value !== id)
}
async function inspect() {
  if (!allowed.value || busy.value || !choices.value.length) return
  const mine = generation
  preparedHead = props.proposal.head
  open.value = busy.value = true
  failed.value = false
  error.value = ''
  previews.value = []
  controller = new AbortController()
  try {
    const result = await previewSuggestions(api, props.repo, preparedHead, choices.value, {
      token: auth.token,
      signal: controller.signal,
      quietUnauthenticated: true,
    })
    if (mine === generation) previews.value = result
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = false
  }
}
async function apply() {
  if (!allowed.value || busy.value || failed.value || !previews.value.length) return
  const mine = generation
  busy.value = true
  error.value = ''
  controller = new AbortController()
  try {
    await applySuggestions(
      api,
      props.repo,
      props.proposal.id,
      props.proposal.stream,
      preparedHead,
      previews.value,
      {
        token: auth.token,
        signal: controller.signal,
        quietUnauthenticated: true,
      },
    )
    if (mine !== generation) return
    reset()
    emit('applied')
  } catch (cause) {
    if (mine === generation) {
      failed.value = true
      error.value = describeError(cause)
    }
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <details v-if="allowed && candidates.length > 1" class="batch">
    <summary>Apply several suggestions</summary>
    <p>Choose suggestions from the loaded discussions. They will become one new revision.</p>
    <ul class="choices">
      <li v-for="item in candidates" :key="item.id">
        <Checkbox
          :model-value="selected.includes(item.id)"
          :label="`${item.anchor.path}, ${item.lines ? `lines ${item.anchor.line}–${item.anchor.line + item.lines - 1}` : `line ${item.anchor.line}`}`"
          :disabled="busy"
          @update:model-value="(value) => choose(item.id, value)"
        />
        <pre v-if="item.replacement">{{ item.replacement }}</pre>
        <p v-else>Delete this line.</p>
      </li>
    </ul>
    <Button :disabled="!choices.length" @click="inspect">Review selected suggestions</Button>
  </details>
  <Dialog v-model="open" title="Apply selected suggestions?" :dismissable="!busy" size="lg">
    <p>
      {{ choices.length }} suggestions in {{ previews.length }} files. Checks and approvals must
      match the new revision. Discussions stay open.
    </p>
    <p v-if="busy" role="status">Preparing the change…</p>
    <Alert v-if="error" tone="bad" live="alert">
      {{ error }}
      <p v-if="failed">Close this preview and inspect the current revision before trying again.</p>
    </Alert>
    <section v-for="preview in previews" :key="preview.change.path" class="file-preview">
      <h3>{{ preview.change.path }}</h3>
      <details>
        <summary>Current file</summary>
        <pre>{{ preview.before }}</pre>
      </details>
      <details open>
        <summary>Prepared file</summary>
        <pre>{{ preview.after }}</pre>
      </details>
    </section>
    <template #footer>
      <Button :disabled="busy" @click="open = false">Cancel</Button>
      <Button
        variant="primary"
        :disabled="!allowed || !previews.length || failed"
        :loading="busy"
        @click="apply"
        >Apply selected suggestions</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.batch {
  padding: var(--s-4);
  background: var(--surface-2);
  border-radius: var(--r-sm);
}
summary {
  cursor: pointer;
  font-weight: 600;
}
.batch > p {
  margin: var(--s-3) 0;
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.choices {
  list-style: none;
  padding: 0;
  margin: var(--s-3) 0;
  display: grid;
  gap: var(--s-4);
}
pre {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  max-height: 20rem;
  overflow: auto;
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
}
.choices pre {
  margin: var(--s-2) 0 0 var(--s-5);
}
.file-preview {
  margin-top: var(--s-4);
  padding-top: var(--s-3);
  border-top: 1px solid var(--border);
}
.file-preview details {
  margin-top: var(--s-3);
}
</style>
