<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Repo } from '@/api/types'
import { formatDate, plural } from '@/lib/format'
import { mapLimit } from '@/lib/pool'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import { usePageTitle } from '@/composables/usePageTitle'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const auth = useAuth()
const toasts = useToasts()
const router = useRouter()
usePageTitle(() => 'Repositories')

const MAX_REPOS = 1000
const all = ref<Repo[]>([])
const counts = ref<Record<string, { streams: number; changesets: number }>>({})
const loading = ref(true)
const truncated = ref(false)
const error = ref<unknown>(null)
const search = ref('')
let serial = 0

const columns: Column[] = [
  { key: 'name', label: 'Repository', width: 'minmax(0, 2.4fr)' },
  { key: 'default_stream', label: 'Default stream', width: '9rem', hideBelow: 760 },
  { key: 'stream_count', label: 'Streams', width: '5.5rem', align: 'end', hideBelow: 900 },
  { key: 'changeset_count', label: 'Changesets', width: '6.5rem', align: 'end', hideBelow: 900 },
  { key: 'created_at_ms', label: 'Created', width: '7.5rem', hideBelow: 1000 },
]

const visible = computed(() => {
  const q = search.value.trim().toLowerCase()
  if (!q) return all.value
  return all.value.filter(
    (r) => r.name.toLowerCase().includes(q) || r.description.toLowerCase().includes(q),
  )
})

// The list endpoint does not fill in counts (describe does), so fetch them for what is on screen.
async function enrich(repos: Repo[], mine: number) {
  const todo = repos.filter((r) => !counts.value[r.name])
  await mapLimit(todo, 4, async (r) => {
    // Stop when this list was replaced or left, or the session ended (no request without a credential).
    if (mine !== serial || !auth.isAuthenticated) return
    const full = await api.repos.get(r.name)
    if (mine !== serial) return
    counts.value = {
      ...counts.value,
      [r.name]: { streams: full.stream_count, changesets: full.changeset_count },
    }
  })
}

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
    void enrich(items.slice(0, 60), mine)
  } catch (e) {
    if (mine !== serial) return
    error.value = e
    loading.value = false
  }
}

// Fetch counts for rows that become visible through searching.
watch(visible, (v) => void enrich(v.slice(0, 60), serial))

onMounted(load)
onBeforeUnmount(() => serial++)

// ---- create ----------------------------------------------------------------------------------------
const creating = ref(false)
const form = ref({ name: '', description: '', stream: 'main' })
const formError = ref<ApiError | null>(null)
const busy = ref(false)
const canCreate = computed(() => auth.isAdminAnywhere)

function openCreate() {
  form.value = { name: '', description: '', stream: 'main' }
  formError.value = null
  creating.value = true
}

async function create() {
  busy.value = true
  formError.value = null
  try {
    const repo = await api.repos.create({
      name: form.value.name.trim(),
      description: form.value.description.trim(),
      default_stream: form.value.stream.trim() || undefined,
    })
    creating.value = false
    toasts.ok('Repository created', repo.name)
    await router.push({
      name: 'board',
      params: { owner: repo.name.split('/')[0], name: repo.name.split('/')[1] },
    })
  } catch (e) {
    formError.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}

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
        :disabled="!canCreate"
        disabled-reason="Creating repositories requires the admin role"
        @click="openCreate"
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
        <template #cell-stream_count="{ row }">{{
          counts[(row as Repo).name]?.streams ?? '–'
        }}</template>
        <template #cell-changeset_count="{ row }">{{
          counts[(row as Repo).name]?.changesets ?? '–'
        }}</template>
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
            <Button v-if="canCreate && !search" variant="primary" icon="plus" @click="openCreate"
              >Create the first one</Button
            >
          </EmptyState>
        </template>
      </DataTable>
    </Card>

    <Dialog
      v-model="creating"
      title="New repository"
      description="Repository names look like owner/name."
      :dismissable="!busy"
    >
      <form id="create-repo" class="form" @submit.prevent="create">
        <TextField
          v-model="form.name"
          label="Name"
          mono
          required
          placeholder="acme/webapp"
          hint="Lowercase letters, digits, “.”, “_” and “-”, as owner/name."
          autofocus
        />
        <TextArea
          v-model="form.description"
          label="Description"
          :rows="2"
          placeholder="What lives here"
        />
        <TextField
          v-model="form.stream"
          label="Default stream"
          mono
          hint="The line of integration most people follow."
        />
        <Alert v-if="formError" tone="bad" live="alert" title="Could not create the repository">
          {{ formError.message }}
        </Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="creating = false">Cancel</Button>
        <Button type="submit" form="create-repo" variant="primary" :loading="busy"
          >Create repository</Button
        >
      </template>
    </Dialog>
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
