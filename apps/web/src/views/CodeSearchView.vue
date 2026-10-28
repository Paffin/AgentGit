<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import type { SearchMatch, SearchTreeResponse } from '@/api/types'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { describeError } from '@/api/errors'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Checkbox from '@/ui/Checkbox.vue'
import EmptyState from '@/ui/EmptyState.vue'
import TextField from '@/ui/TextField.vue'

const { repo, routeParams } = useRepoName()
const route = useRoute()
const router = useRouter()
const auth = useAuth()
const board = useBoard()
usePageTitle(() => `Search code · ${repo.value}`)
const param = (key: string) =>
  typeof route.query[key] === 'string' ? (route.query[key] as string) : ''
const query = ref(param('q'))
const revision = ref(param('rev'))
const prefix = ref(param('prefix'))
const matchCase = ref(param('case') === '1')
const supported = computed(() => auth.capabilities?.features.includes('tree.search') === true)
const results = ref<SearchMatch[]>([])
const page = ref<SearchTreeResponse | null>(null)
const skippedBinary = ref(0)
const skippedLarge = ref(0)
const busy = ref(false)
const error = ref('')
let context = 0
let controller: AbortController | undefined
const groups = computed(() => {
  const byPath = new Map<string, SearchMatch[]>()
  for (const match of results.value) {
    const matches = byPath.get(match.path)
    if (matches) matches.push(match)
    else byPath.set(match.path, [match])
  }
  return [...byPath].map(([path, matches]) => ({ path, matches }))
})
const target = (match: SearchMatch) => ({
  name: 'files',
  params: routeParams.value,
  query: { rev: page.value?.rev, path: match.path, line: String(match.line) },
})
async function run(append = false) {
  if (busy.value || !supported.value || !param('q')) return
  const mine = context
  const q = param('q')
  const rev = append ? page.value?.rev : param('rev') || board.repo?.default_stream
  if (!rev) return
  if (new TextEncoder().encode(q).length > 256) {
    error.value = 'Search text must be at most 256 UTF-8 bytes.'
    return
  }
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    const response = await api.tree.search(
      repo.value,
      {
        query: q,
        rev,
        path_prefix: param('prefix') || undefined,
        case_sensitive: param('case') === '1',
        page_size: 50,
        page_token: append ? page.value?.next_page_token : undefined,
      },
      { signal: controller.signal },
    )
    if (mine !== context) return
    if (append && page.value && response.tree_id !== page.value.tree_id)
      throw new Error('The search revision changed. Start the search again.')
    results.value = append ? [...results.value, ...response.matches] : response.matches
    page.value = response
    skippedBinary.value += response.skipped_binary
    skippedLarge.value += response.skipped_large
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  } finally {
    if (mine === context) busy.value = false
  }
}
function restart() {
  context++
  controller?.abort()
  busy.value = false
  results.value = []
  page.value = null
  skippedBinary.value = 0
  skippedLarge.value = 0
  error.value = ''
  void run()
}
function submit() {
  const next = {
    q: query.value.trim() || undefined,
    rev: revision.value.trim() || undefined,
    prefix: prefix.value.trim() || undefined,
    case: matchCase.value ? '1' : undefined,
  }
  if (
    next.q === (param('q') || undefined) &&
    next.rev === (param('rev') || undefined) &&
    next.prefix === (param('prefix') || undefined) &&
    next.case === (param('case') || undefined)
  )
    restart()
  else void router.push({ query: next })
}
watch(
  [repo, () => route.query, supported, () => board.repo?.default_stream],
  () => {
    query.value = param('q')
    revision.value = param('rev')
    prefix.value = param('prefix')
    matchCase.value = param('case') === '1'
    restart()
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
})
</script>

<template>
  <div class="search-page">
    <header>
      <h1>Search code</h1>
      <p class="muted">
        Find text across files at one revision. Search text is literal, not a regular expression.
      </p>
    </header>
    <Alert v-if="auth.capabilities && !supported" tone="warn" title="Server update required"
      >This server does not support code search yet. Update it to enable this page.</Alert
    >
    <form class="search-form" @submit.prevent="submit">
      <TextField
        v-model="query"
        type="search"
        label="Search text"
        placeholder="Function name, error message or symbol"
      />
      <div class="options">
        <TextField
          v-model="revision"
          label="Branch or revision"
          :placeholder="board.repo?.default_stream || 'main'"
        /><TextField
          v-model="prefix"
          label="File or folder (optional)"
          placeholder="src/"
        /><Checkbox v-model="matchCase" label="Match case" />
      </div>
      <Button
        type="submit"
        variant="primary"
        :loading="busy"
        :disabled="!supported || !query.trim()"
        >Search</Button
      >
    </form>
    <Alert v-if="error" tone="bad" live="alert" title="Search could not finish"
      >{{ error }}
      <Button size="sm" @click="page?.next_page_token ? run(true) : restart()"
        >Try again</Button
      ></Alert
    >
    <p v-if="page" class="muted" role="status">
      For <code>{{ param('q') }}</code
      >: {{ results.length }} matching {{ results.length === 1 ? 'line' : 'lines' }} found{{
        page.next_page_token ? ' so far' : ''
      }}
      at <code>{{ page.rev.slice(0, 12) || 'empty branch' }}</code
      >.
      {{
        page.next_page_token
          ? 'More files or lines remain to be searched.'
          : 'Search finished for eligible text files.'
      }}
    </p>
    <Alert v-if="skippedBinary || skippedLarge" tone="warn" title="Some files were excluded"
      >{{ skippedBinary }} binary or non-UTF-8 {{ skippedBinary === 1 ? 'file' : 'files' }} and
      {{ skippedLarge }} {{ skippedLarge === 1 ? 'file' : 'files' }} over 1 MiB were skipped.</Alert
    >
    <p class="muted hint">
      Text files and symbolic-link targets up to 1 MiB are searched. Links are not followed. Results
      stay on the same revision while you continue.
    </p>
    <EmptyState
      v-if="!param('q')"
      icon="search"
      title="Search file contents"
      description="Enter text to find where it appears in this repository."
    />
    <EmptyState
      v-else-if="page && !results.length"
      icon="search"
      :title="page.next_page_token ? 'No matches in this part yet' : 'No matching text'"
      :description="
        page.next_page_token
          ? 'Continue searching the remaining files.'
          : 'Try a shorter phrase, a different revision or a broader folder.'
      "
    />
    <section
      v-for="group in groups"
      :key="group.path"
      class="result-file"
      :aria-label="`Matches in ${group.path}`"
    >
      <h2>{{ group.path }}</h2>
      <ol>
        <li v-for="match in group.matches" :key="match.line">
          <RouterLink :to="target(match)"
            ><span class="line-number">Line {{ match.line }}</span
            ><code>{{ match.text }}</code></RouterLink
          >
        </li>
      </ol>
    </section>
    <Button v-if="page?.next_page_token" :loading="busy" @click="run(true)"
      >Continue searching</Button
    >
  </div>
</template>

<style scoped>
.search-page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.search-form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  align-items: stretch;
}
.search-form > :last-child {
  align-self: start;
}
.options {
  display: flex;
  align-items: end;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.options > :first-child,
.options > :nth-child(2) {
  flex: 1;
  min-width: 180px;
}
.result-file {
  border-top: 1px solid var(--border);
  padding-top: var(--s-3);
}
.result-file h2 {
  font-size: var(--fs-md);
  overflow-wrap: anywhere;
  margin-bottom: var(--s-2);
}
ol {
  list-style: none;
  margin: 0;
  padding: 0;
}
li a {
  display: grid;
  grid-template-columns: 6rem minmax(0, 1fr);
  gap: var(--s-3);
  padding: var(--s-2);
  text-decoration: none;
  color: var(--text);
  border-radius: var(--r-sm);
}
li a:hover,
li a:focus-visible {
  background: var(--surface-2);
}
.line-number {
  color: var(--text-3);
  font-size: var(--fs-sm);
}
li code {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.hint {
  font-size: var(--fs-xs);
}
</style>
