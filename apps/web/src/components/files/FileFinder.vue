<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import type { TreeEntry } from '@/api/types'
import { isApiError } from '@/api/errors'
import Dialog from '@/ui/Dialog.vue'
import TextField from '@/ui/TextField.vue'
import Button from '@/ui/Button.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'

const props = defineProps<{ modelValue: boolean; repo: string; rev: string; branch?: boolean }>()
const emit = defineEmits<{ 'update:modelValue': [value: boolean]; select: [path: string] }>()
const query = ref('')
const entries = ref<TreeEntry[]>([])
const total = ref(0)
const next = ref('')
const revision = ref('')
const busy = ref(false)
const error = ref<unknown>(null)
let controller: AbortController | undefined
const matches = computed(() => {
  const parts = query.value.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean)
  return entries.value.filter((entry) =>
    parts.every((part) => entry.path.toLocaleLowerCase().includes(part)),
  )
})
const shown = computed(() => matches.value.slice(0, 100))

async function resolveRevision(
  repository: string,
  rev: string,
  signal: AbortSignal,
): Promise<string> {
  if (props.branch) return (await api.streams.head(repository, rev, { signal })).head
  try {
    return await api.history.resolve(repository, rev, { signal })
  } catch (error) {
    if (!isApiError(error) || error.status !== 404) throw error
    return (await api.streams.head(repository, rev, { signal })).head
  }
}

async function load(reset = false) {
  controller?.abort()
  const repository = props.repo
  const source = props.rev
  const current = new AbortController()
  controller = current
  busy.value = true
  error.value = null
  if (reset) {
    entries.value = []
    total.value = 0
    next.value = ''
    revision.value = ''
  }
  try {
    if (!revision.value) {
      const resolved = await resolveRevision(repository, source, current.signal)
      if (current.signal.aborted) return
      revision.value = resolved
    }
    if (!revision.value) return
    await loadPages(repository, current.signal)
  } catch (e) {
    if (!current.signal.aborted) error.value = e
  } finally {
    if (controller === current) busy.value = false
  }
}
async function loadPages(repository: string, signal: AbortSignal) {
  const target = entries.value.length + 10_000
  let pages = 0
  do {
    const page = await api.tree.manifest(
      repository,
      { rev: revision.value, page_size: 1000, page_token: next.value || undefined },
      { signal },
    )
    if (signal.aborted) return
    if (page.nextPageToken && page.nextPageToken === next.value)
      throw new Error('The file list did not advance. Please retry.')
    entries.value = [...entries.value, ...page.entries]
    total.value = page.total
    next.value = page.nextPageToken
    pages++
  } while (next.value && entries.value.length < target && pages < 20)
}
watch(
  () => [props.modelValue, props.repo, props.rev],
  () => {
    if (props.modelValue) {
      if (props.rev) void load(true)
      else busy.value = true
    } else controller?.abort()
  },
  { immediate: true },
)
watch(
  () => props.modelValue,
  (open) => {
    if (open) query.value = ''
  },
)
onBeforeUnmount(() => controller?.abort())
function choose(path: string) {
  emit('update:modelValue', false)
  emit('select', path)
}
function follow(event: MouseEvent, path: string) {
  if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return
  event.preventDefault()
  choose(path)
}
</script>

<template>
  <Dialog
    :model-value="modelValue"
    title="Find a file"
    description="Search file names and paths across this repository."
    size="lg"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="finder">
      <TextField
        v-model="query"
        label="File name or path"
        type="search"
        placeholder="For example: src auth"
        autofocus
        @keydown.enter.prevent="shown[0] && choose(shown[0].path)"
      />
      <p class="faint" role="status">
        {{ entries.length.toLocaleString()
        }}{{ total ? ` of ${total.toLocaleString()}` : '' }} files loaded<span v-if="busy">
          · Loading…</span
        >
        <span v-if="matches.length > 100">
          · Showing the first 100 matches; narrow your search.</span
        >
      </p>
      <ErrorState
        v-if="error"
        what="the file list"
        :error="error"
        compact
        @retry="load(!revision)"
      />
      <ul class="results" aria-label="Matching files">
        <li v-for="entry in shown" :key="entry.path">
          <a
            :href="`/repos/${repo}/files?rev=${encodeURIComponent(rev)}&path=${encodeURIComponent(entry.path)}`"
            @click="follow($event, entry.path)"
          >
            <Icon name="file" :size="16" /><span class="mono">{{ entry.path }}</span>
          </a>
        </li>
      </ul>
      <p v-if="!shown.length && !busy && !error" class="muted">
        {{
          entries.length
            ? 'No matching files. Try part of a file name or path.'
            : 'This revision has no files.'
        }}
      </p>
      <Button v-if="next && !busy" @click="load()">Load more files</Button>
    </div>
  </Dialog>
</template>

<style scoped>
.finder {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.results {
  list-style: none;
  margin: 0;
  padding: 0;
  max-height: 50vh;
  overflow: auto;
}
.results a {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  min-height: var(--touch-min);
  padding: var(--s-2);
  color: var(--text);
  border-radius: var(--r-sm);
}
.results a:hover,
.results a:focus-visible {
  background: var(--surface-2);
  color: var(--accent-text);
}
.results span {
  overflow-wrap: anywhere;
  min-width: 0;
}
.results svg {
  flex: none;
}
</style>
