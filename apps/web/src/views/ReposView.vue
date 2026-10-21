<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
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
import type { Column } from '@/ui/types'

const auth = useAuth()
usePageTitle(() => 'Repositories')

const MAX_REPOS = 1000
const all = ref<Repo[]>([])
const loading = ref(true)
const truncated = ref(false)
const error = ref<unknown>(null)
const search = ref('')
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

async function load() {
  const mine = ++serial
  loading.value = true
  error.value = null
  truncated.value = false
  try {
    const items: Repo[] = []
    let token = ''
    do {
      const page = await api.repos.list({ page_size: 200, page_token: token || undefined })
      if (mine !== serial) return
      items.push(...page.items)
      token = page.nextPageToken
      all.value = [...items]
      if (items.length >= MAX_REPOS && token) {
        truncated.value = true
        break
      }
    } while (token)
    loading.value = false
  } catch (e) {
    if (mine !== serial) return
    error.value = e
    loading.value = false
  }
}

onMounted(load)
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
        v-model="search"
        label="Search repositories"
        hide-label
        type="search"
        placeholder="Search by name or description"
        class="search"
      >
        <template #prefix><span class="mag" aria-hidden="true">⌕</span></template>
      </TextField>
      <span v-if="!loading" class="faint count" aria-live="polite">{{
        plural(visible.length, 'repository', 'repositories')
      }}</span>
    </div>

    <Alert v-if="truncated" tone="info"
      >Showing the first {{ MAX_REPOS }} repositories. Use the search to narrow the list.</Alert
    >

    <Card flush>
      <ErrorState v-if="error" what="repositories" :error="error" @retry="load" />
      <DataTable
        v-else
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
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
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
</style>
