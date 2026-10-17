<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter, RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError } from '@/api/errors'
import type { DirEntry } from '@/api/types'
import FileViewer from '@/components/files/FileViewer.vue'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { useResource } from '@/composables/useResource'
import { formatBytes } from '@/lib/format'
import { useBoard } from '@/stores/board'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import LoadMore from '@/ui/LoadMore.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const { repo } = useRepoName()
const route = useRoute()
const router = useRouter()
const board = useBoard()
usePageTitle(() => `Files · ${repo.value}`)

const qRev = computed(() => (typeof route.query.rev === 'string' ? route.query.rev : ''))
const path = computed(() =>
  typeof route.query.path === 'string' ? route.query.path.replace(/^\/+|\/+$/g, '') : '',
)
const rev = computed(() => qRev.value || board.repo?.default_stream || 'main')
const draft = ref(qRev.value)
watch(qRev, (v) => (draft.value = v))

// What is at this path? (A directory, a file, or nothing.)
const where = useResource<{ kind: string }>(
  async () => {
    if (!path.value) return { kind: 'dir' }
    return api.tree.resolve(repo.value, rev.value, path.value)
  },
  { watch: [repo, rev, path] },
)

const entries = ref<DirEntry[]>([])
const next = ref('')
const listing = ref(false)
const more = ref(false)
const listError = ref<unknown>(null)
const emptyRepo = ref(false)
let serial = 0
async function list(append = false) {
  const mine = ++serial
  if (append) more.value = true
  else listing.value = true
  listError.value = null
  emptyRepo.value = false
  try {
    const page = await api.tree.list(repo.value, {
      rev: rev.value,
      path: path.value || undefined,
      page_size: 200,
      page_token: append ? next.value : undefined,
    })
    if (mine !== serial) return
    entries.value = append ? [...entries.value, ...page.entries] : page.entries
    next.value = page.nextPageToken
  } catch (e) {
    if (mine !== serial) return
    if (e instanceof ApiError && e.isNotFound && /no changesets yet/.test(e.message)) {
      emptyRepo.value = true
      entries.value = []
    } else listError.value = e
  } finally {
    if (mine === serial) {
      listing.value = false
      more.value = false
    }
  }
}
watch(
  [() => where.data.value?.kind, rev, path, repo],
  ([kind]) => {
    if (kind === 'dir') void list()
  },
  { immediate: true },
)

const crumbs = computed(() => {
  const parts = path.value ? path.value.split('/') : []
  return parts.map((p, i) => ({ name: p, path: parts.slice(0, i + 1).join('/') }))
})
const to = (p: string) => ({
  path: `/repos/${repo.value}/files`,
  query: { rev: qRev.value || undefined, path: p || undefined },
})

const sorted = computed(() =>
  [...entries.value].sort(
    (a, b) => Number(b.kind === 'dir') - Number(a.kind === 'dir') || a.name.localeCompare(b.name),
  ),
)
const anySize = computed(() => entries.value.some((e) => e.size > 0))
const columns = computed<Column[]>(() => [
  { key: 'name', label: 'Name', width: 'minmax(0, 1fr)' },
  ...(anySize.value ? [{ key: 'size', label: 'Size', width: '7rem', align: 'end' as const }] : []),
  { key: 'kind', label: 'Kind', width: '7rem', hideBelow: 700 },
])
const fullPath = (name: string) => (path.value ? `${path.value}/${name}` : name)

function applyRev() {
  void router.push({ query: { ...route.query, rev: draft.value.trim() || undefined } })
}
const streamNames = computed(() => board.streams.map((s) => s.name))
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>Files</h1>
        <p class="muted">
          The tree at <code class="mono">{{ rev }}</code
          >, read from the server. Read-only.
        </p>
      </div>
      <form class="rev" @submit.prevent="applyRev">
        <TextField
          v-model="draft"
          label="Revision"
          mono
          :placeholder="rev"
          list="rev-list"
          hint="Stream, changeset id or main~2"
        />
        <Button type="submit" icon="search">Open</Button>
        <datalist id="rev-list">
          <option v-for="s in streamNames" :key="s" :value="s" />
        </datalist>
      </form>
    </div>

    <Card flush>
      <template #header>
        <nav class="crumbs" aria-label="Path">
          <RouterLink :to="to('')" class="mono">{{ repo }}</RouterLink>
          <template v-for="c in crumbs" :key="c.path">
            <Icon name="chevron-right" :size="13" class="sep" />
            <RouterLink
              :to="to(c.path)"
              class="mono"
              :aria-current="c.path === path ? 'page' : undefined"
              >{{ c.name }}</RouterLink
            >
          </template>
        </nav>
      </template>

      <ErrorState
        v-if="where.error.value"
        what="this path"
        :error="where.error.value"
        @retry="where.reload()"
      />
      <div v-else-if="where.data.value?.kind === 'missing'" class="pad">
        <EmptyState
          icon="search"
          title="Nothing at this path"
          :description="`“${path}” does not exist at ${rev}.`"
        >
          <Button :to="to('')">Back to the root</Button>
        </EmptyState>
      </div>
      <div v-else-if="where.data.value && where.data.value.kind !== 'dir'" class="pad">
        <FileViewer :repo="repo" :rev="rev" :path="path" />
      </div>
      <template v-else>
        <ErrorState v-if="listError" what="the directory" :error="listError" @retry="list()" />
        <EmptyState
          v-else-if="emptyRepo"
          icon="folder"
          title="This stream has no files yet"
          description="Nothing has been integrated into it."
        />
        <template v-else>
          <DataTable
            :columns="columns"
            :rows="sorted"
            :row-key="(e: DirEntry) => e.name"
            caption="Directory listing"
            :loading="listing || !where.settled.value"
            dense
          >
            <template #cell-name="{ row }">
              <RouterLink :to="to(fullPath((row as DirEntry).name))" class="entry">
                <Icon :name="(row as DirEntry).kind === 'dir' ? 'folder' : 'file'" :size="16" />
                <span class="mono">{{ (row as DirEntry).name }}</span>
                <Badge v-if="(row as DirEntry).executable" size="sm">executable</Badge>
                <Badge v-if="(row as DirEntry).kind === 'symlink'" size="sm" tone="info"
                  >symlink</Badge
                >
              </RouterLink>
            </template>
            <template #cell-size="{ row }"
              ><span class="faint">{{
                (row as DirEntry).size ? formatBytes((row as DirEntry).size) : ''
              }}</span></template
            >
            <template #cell-kind="{ row }"
              ><span class="faint">{{ (row as DirEntry).kind }}</span></template
            >
            <template #empty
              ><EmptyState icon="folder" compact title="This directory is empty"
            /></template>
          </DataTable>
          <LoadMore
            :has-more="!!next"
            :loading="more"
            :shown="sorted.length"
            noun="entries"
            @more="list(true)"
          />
        </template>
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
.head {
  display: flex;
  justify-content: space-between;
  align-items: flex-end;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.rev {
  display: flex;
  gap: var(--s-2);
  align-items: flex-end;
}
.rev :deep(.field) {
  width: 280px;
  max-width: 60vw;
}
.crumbs {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
  padding: var(--s-3) var(--s-4);
  width: 100%;
  font-size: var(--fs-sm);
}
.sep {
  color: var(--text-3);
}
.pad {
  padding: var(--s-4);
}
.entry {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--text);
}
.entry:hover {
  color: var(--accent-text);
}
</style>
