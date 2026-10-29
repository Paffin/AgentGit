<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import type { Repo } from '@/api/types'
import { formatDate, plural } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import { usePageTitle } from '@/composables/usePageTitle'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import TextField from '@/ui/TextField.vue'
import LoadMore from '@/ui/LoadMore.vue'
import Icon from '@/ui/Icon.vue'
import type { Column } from '@/ui/types'

const auth = useAuth()
usePageTitle(() => 'Repositories')

const MAX_REPOS = 1000
const all = ref<Repo[]>([])
const loading = ref(true)
const next = ref('')
const error = ref<unknown>(null)
const route = useRoute()
const router = useRouter()
const readSearch = (value: unknown): string => (typeof value === 'string' ? value : '')
const search = ref(readSearch(route.query.q))
watch(
  () => route.query.q,
  (value) => {
    search.value = readSearch(value)
  },
)
function updateSearch(value: string) {
  search.value = value
  void router.replace({ query: { ...route.query, q: value || undefined } })
}
let serial = 0

const columns: Column[] = [
  { key: 'name', label: 'Repository', width: 'minmax(0, 2.4fr)' },
  { key: 'default_stream', label: 'Main branch', width: '9rem', hideBelow: 760 },
  { key: 'stream_count', label: 'Branches', width: '5.5rem', align: 'end', hideBelow: 900 },
  { key: 'changeset_count', label: 'Versions', width: '6.5rem', align: 'end', hideBelow: 900 },
  { key: 'created_at_ms', label: 'Created', width: '7.5rem', hideBelow: 1000 },
]

const visible = computed(() => {
  const q = search.value.trim().toLowerCase()
  if (!q) return all.value
  return all.value.filter(
    (r) => r.name.toLowerCase().includes(q) || r.description.toLowerCase().includes(q),
  )
})

// The list carries the real counts. A repository whose metadata could not be read says so (`counts_error`)
// and shows a dash with the reason, never a zero.
const countText = (r: Repo, n: number) => (r.counts_error ? '–' : String(n))

async function load(append = false) {
  const mine = ++serial
  loading.value = true
  error.value = null
  if (!append) next.value = ''
  try {
    await loadPages(append ? [...all.value] : [], append ? next.value : '', mine)
    if (mine !== serial) return
    loading.value = false
  } catch (e) {
    if (mine !== serial) return
    error.value = e
    loading.value = false
  }
}

async function loadPages(items: Repo[], token: string, mine: number) {
  const stopAt = items.length + MAX_REPOS
  let pages = 0
  do {
    const page = await api.repos.list({ page_size: 200, page_token: token || undefined })
    if (mine !== serial) return
    if (page.nextPageToken && page.nextPageToken === token)
      throw new Error('The repository list did not advance. Please retry.')
    items.push(...page.items)
    token = page.nextPageToken
    all.value = [...items]
    next.value = token
    if (items.length >= stopAt || ++pages >= 5) break
  } while (token)
}

onMounted(() => load())
onBeforeUnmount(() => serial++)

const canCreate = computed(() => auth.isAdminAnywhere)

const split = (name: string) => {
  const [owner = '', repo = ''] = name.split('/')
  return { owner, name: repo }
}
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>Repositories</h1>
        <p class="muted">Every repository you may read. Open one to watch its agents work.</p>
      </div>
      <Button v-if="canCreate" to="/import">Import from GitHub / GitLab</Button>
      <Button
        variant="primary"
        icon="plus"
        to="/repos/new"
        :disabled="!canCreate"
        disabled-reason="Creating repositories requires the admin role"
      >
        New repository
      </Button>
    </div>

    <div class="tools">
      <TextField
        :model-value="search"
        label="Search repositories"
        hide-label
        type="search"
        placeholder="Search by name or description"
        class="search"
        @update:model-value="updateSearch"
      >
        <template #prefix><Icon name="search" :size="16" /></template>
      </TextField>
      <span v-if="!loading" class="faint count" aria-live="polite">{{
        plural(visible.length, 'repository', 'repositories')
      }}</span>
    </div>

    <Alert v-if="next" tone="info"
      >Search matches the {{ all.length }} loaded repositories. Load more to include the
      rest.</Alert
    >

    <Card flush class="repository-list">
      <ErrorState v-if="error" what="repositories" :error="error" @retry="load(all.length > 0)" />
      <DataTable
        v-if="!error || all.length > 0"
        :columns="columns"
        :rows="visible"
        :row-key="(r: Repo) => r.name"
        caption="Repositories"
        :loading="loading"
        :skeleton-rows="5"
      >
        <template #cell-name="{ row }">
          <RouterLink :to="{ name: 'board', params: split((row as Repo).name) }" class="repo-link">
            <strong class="mono">{{ (row as Repo).name }}</strong>
          </RouterLink>
          <p v-if="(row as Repo).description" class="desc">
            {{ (row as Repo).description }}
          </p>
        </template>
        <template #cell-default_stream="{ row }"
          ><Badge mono icon="branch">{{ (row as Repo).default_stream }}</Badge></template
        >
        <template #cell-stream_count="{ row }"
          ><span :title="(row as Repo).counts_error || undefined">{{
            countText(row as Repo, (row as Repo).stream_count)
          }}</span></template
        >
        <template #cell-changeset_count="{ row }"
          ><span :title="(row as Repo).counts_error || undefined">{{
            countText(row as Repo, (row as Repo).changeset_count)
          }}</span></template
        >
        <template #cell-created_at_ms="{ row }"
          ><span class="faint">{{ formatDate((row as Repo).created_at_ms) }}</span></template
        >
        <template #empty>
          <EmptyState
            icon="database"
            :title="search ? 'No repository matches' : 'No repositories yet'"
            :description="
              search
                ? 'Try a different search, or check the spelling of the owner.'
                : 'You can see a repository once an administrator grants you a role on it.'
            "
          >
            <Button v-if="canCreate && !search" variant="primary" icon="plus" to="/repos/new"
              >Create the first one</Button
            >
          </EmptyState>
        </template>
      </DataTable>
      <LoadMore
        v-if="all.length || next"
        :has-more="!!next"
        :loading="loading"
        :shown="all.length"
        noun="repositories loaded"
        @more="load(true)"
      />
    </Card>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  align-items: center;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.tools {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}
.search {
  flex: 1;
  max-width: 420px;
}
.mag {
  color: var(--text-3);
}
.count {
  font-size: var(--fs-sm);
}
.repo-link {
  color: var(--text);
}
.repo-link:hover strong {
  color: var(--accent-text);
}
.desc {
  color: var(--text-2);
  font-size: var(--fs-sm);
  margin-top: 2px;
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.head p {
  margin-top: var(--s-2);
  font-size: var(--fs-md);
}
.tools {
  padding: var(--s-4) 0;
  border-bottom: 1px solid var(--border);
}
.repository-list {
  border: 0;
  box-shadow: none;
  border-radius: var(--r-md);
}
.repository-list :deep(th) {
  padding-top: var(--s-3);
  padding-bottom: var(--s-3);
}
.repo-link {
  font-size: var(--fs-lg);
}
@media (max-width: 960px) {
  .head {
    grid-template-columns: minmax(0, 1fr) auto;
  }
  .head > div {
    grid-column: 1 / -1;
  }
  .head > .btn {
    justify-self: start;
  }
}
@media (max-width: 600px) {
  .tools {
    flex-direction: column;
    align-items: stretch;
  }
  .search {
    width: 100%;
    max-width: none;
  }
  .head {
    display: flex;
    align-items: flex-start;
  }
  .head > div {
    width: 100%;
  }
}
</style>
