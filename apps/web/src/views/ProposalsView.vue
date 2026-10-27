<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import type { Proposal } from '@/api/types'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { shortId } from '@/lib/format'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Icon from '@/ui/Icon.vue'
import { useAuth } from '@/stores/auth'
import { termTip } from '@/lib/glossary'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import LoadMore from '@/ui/LoadMore.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Segmented from '@/ui/Segmented.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const { repo } = useRepoName()
usePageTitle(() => `Proposals · ${repo.value}`)

type StateFilter = 'open' | 'integrated' | 'closed' | 'all'
const state = ref<StateFilter>('open')
const author = ref('')
const query = ref('')
const auth = useAuth()
const items = ref<Proposal[]>([])
const next = ref('')
const loading = ref(true)
const more = ref(false)
const error = ref<unknown>(null)
let serial = 0

const columns: Column[] = [
  {
    key: 'title',
    label: 'Change request',
    tooltip: termTip('proposal'),
    width: 'minmax(0, 2.4fr)',
  },
  { key: 'author', label: 'Author', width: 'minmax(0, 1.2fr)', hideBelow: 760 },
  { key: 'stream', label: 'Branch', tooltip: termTip('stream'), width: '8rem', hideBelow: 900 },
  { key: 'state', label: 'State', width: '7rem' },
  { key: 'updated_at_ms', label: 'Updated', width: '7rem', align: 'end' },
]

async function load(append = false) {
  const mine = ++serial
  if (append) more.value = true
  else loading.value = true
  error.value = null
  try {
    const page = await api.proposals.list(repo.value, {
      state: state.value === 'all' ? undefined : state.value,
      author: author.value.trim() || undefined,
      page_size: 50,
      page_token: append ? next.value : undefined,
    })
    if (mine !== serial) return
    items.value = append ? [...items.value, ...page.items] : page.items
    next.value = page.nextPageToken
  } catch (e) {
    if (mine !== serial) return
    error.value = e
  } finally {
    if (mine === serial) {
      loading.value = false
      more.value = false
    }
  }
}

let timer: ReturnType<typeof setTimeout> | undefined
watch([state, author, repo], () => {
  clearTimeout(timer)
  timer = setTimeout(() => void load(), 200)
})
onBeforeUnmount(() => {
  serial++
  clearTimeout(timer)
})
void load()

const stateWord = (s: string) => (s === 'integrated' ? 'landed' : s)
const TONE = { open: 'info', integrated: 'ok', closed: 'neutral' } as const
const tone = (s: string) => (s in TONE ? TONE[s as keyof typeof TONE] : 'neutral')
const statusOptions = [
  { value: 'open', label: 'Open' },
  { value: 'integrated', label: 'Landed' },
  { value: 'closed', label: 'Closed' },
  { value: 'all', label: 'All' },
] as const
const rows = computed(() => {
  const words = query.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  return items.value.filter((p) => {
    const text = [p.title, p.author, p.id, p.stream].join(' ').toLocaleLowerCase()
    return words.every((word) => text.includes(word))
  })
})
</script>

<template>
  <div class="page">
    <div class="page-head">
      <div>
        <h1>Change requests</h1>
        <p class="muted">Review proposed changes before they reach the branch.</p>
      </div>
      <div class="actions">
        <Button
          v-if="auth.can(repo, 'editFiles')"
          icon="edit"
          :to="{ name: 'files', params: { owner: repo.split('/')[0], name: repo.split('/')[1] } }"
          >Edit files</Button
        >
        <Button
          v-if="auth.can(repo, 'createTask')"
          variant="primary"
          icon="bot"
          :to="{ name: 'agents', params: { owner: repo.split('/')[0], name: repo.split('/')[1] } }"
          >Give an agent work</Button
        >
      </div>
    </div>
    <div class="tools">
      <Segmented v-model="state" legend="State" :options="statusOptions" />
      <TextField
        v-model="author"
        label="Author"
        placeholder="e.g. agent:refactor-bot"
        hint="Exact identity or principal."
        class="author"
        mono
      />
    </div>
    <TextField
      v-model="query"
      label="Search loaded change requests"
      type="search"
      placeholder="Title, author, branch or ID"
      :hint="
        next ? 'Search covers the loaded page. Load more to include older changes.' : undefined
      "
      ><template #prefix><Icon name="search" :size="16" /></template
    ></TextField>
    <Card flush>
      <ErrorState v-if="error" what="change requests" :error="error" @retry="load()" />
      <template v-else>
        <DataTable
          :columns="columns"
          :rows="rows"
          :row-key="(r: Proposal) => r.id"
          caption="Proposals"
          :loading="loading"
        >
          <template #cell-title="{ row }">
            <RouterLink
              :to="`/repos/${repo}/proposals/${encodeURIComponent((row as Proposal).id)}`"
              class="t"
              >{{ (row as Proposal).title || '(untitled)' }}</RouterLink
            >
            <p class="faint id mono">{{ shortId((row as Proposal).id, 10) }}</p>
          </template>
          <template #cell-author="{ row }"
            ><IdentityLabel :identity="(row as Proposal).author" :size="18"
          /></template>
          <template #cell-stream="{ row }"
            ><Badge mono size="sm" icon="branch">{{ (row as Proposal).stream }}</Badge></template
          >
          <template #cell-state="{ row }"
            ><Badge size="sm" :tone="tone((row as Proposal).state)">{{
              stateWord((row as Proposal).state)
            }}</Badge></template
          >
          <template #cell-updated_at_ms="{ row }"
            ><span class="faint"><RelativeTime :ms="(row as Proposal).updated_at_ms" /></span
          ></template>
          <template #empty>
            <EmptyState
              icon="proposal"
              :title="`No ${state === 'all' ? '' : state + ' '}change requests`"
              description="Nothing matches. Change the filter, or wait for an agent to propose something. To start one yourself, edit a file under Code, or give an agent work."
            />
          </template>
        </DataTable>
        <LoadMore
          :has-more="!!next"
          :loading="more"
          :shown="items.length"
          noun="proposals"
          @more="load(true)"
        />
      </template>
    </Card>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.page-head,
.actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.page-head p {
  margin-top: var(--s-2);
}
.tools {
  display: flex;
  gap: var(--s-4);
  align-items: flex-end;
  flex-wrap: wrap;
}
.author {
  width: 300px;
  max-width: 100%;
}
.t {
  font-weight: 580;
  color: var(--text);
}
.t:hover {
  color: var(--accent-text);
}
.id {
  font-size: var(--fs-xs);
}
</style>
