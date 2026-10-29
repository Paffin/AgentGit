<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import type { RouteLocationRaw } from 'vue-router'
import { useAttention } from '@/composables/useAttention'
import { usePageTitle } from '@/composables/usePageTitle'
import { ATTENTION_HELP, ATTENTION_ORDER, ATTENTION_TITLE, groupAttention } from '@/lib/attention'
import type { AttentionItem, AttentionKind } from '@/lib/attention'
import { filterAttention } from '@/lib/attention-filter'
import { formatDuration, formatTime, plural } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import LiveIndicator from '@/ui/LiveIndicator.vue'
import SelectField from '@/ui/SelectField.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextField from '@/ui/TextField.vue'

const auth = useAuth()
usePageTitle(() => 'Home')
const route = useRoute()
const router = useRouter()
const readText = (value: unknown): string => (typeof value === 'string' ? value : '')
const readKind = (value: unknown) => ATTENTION_ORDER.find((kind) => kind === value)
const repository = ref(readText(route.query.repo))
const {
  repos,
  items,
  loading,
  settled,
  lastUpdated,
  liveStatus,
  error,
  skipped,
  unreadable,
  reload,
  loadMore,
} = useAttention(repository)
const query = ref(readText(route.query.q))
const kind = ref<AttentionKind | undefined>(readKind(route.query.kind))
watch(
  () => route.query,
  () => {
    repository.value = readText(route.query.repo)
    query.value = readText(route.query.q)
    kind.value = readKind(route.query.kind)
  },
)
function updateFilters(values: Partial<{ q: string; repo: string; kind: AttentionKind | '' }>) {
  query.value = values.q ?? query.value
  repository.value = values.repo ?? repository.value
  kind.value = values.kind === '' ? undefined : (values.kind ?? kind.value)
  void router.replace({
    query: {
      ...route.query,
      q: query.value || undefined,
      repo: repository.value || undefined,
      kind: kind.value,
    },
  })
}
const scoped = computed(() =>
  filterAttention(items.value, { query: query.value, repo: repository.value }),
)
const visible = computed(() => filterAttention(scoped.value, { kind: kind.value }))
const groups = computed(() => groupAttention(visible.value))
const repositoryOptions = computed(() => [
  { value: '', label: 'All repositories' },
  ...repos.value.map((r) => ({ value: r.name, label: r.name })),
])
const counts = computed(
  () => new Map(groupAttention(scoped.value).map((g) => [g.kind, g.items.length])),
)
const incomplete = computed(() => Boolean(error.value || unreadable.value || skipped.value))
const filtering = computed(() => Boolean(query.value || repository.value || kind.value))
const total = computed(() => items.value.length)
const canStart = computed(() => auth.isAdminAnywhere)
const first = computed(() => repos.value[0])
const split = (repo: string) => {
  const [owner = '', name = ''] = repo.split('/')
  return { owner, name }
}
function clearFilters() {
  updateFilters({ q: '', repo: '', kind: '' })
}
function target(i: AttentionItem): RouteLocationRaw {
  const { owner, name } = split(i.repo)
  if (!i.proposalId) return { name: 'agents', params: { owner, name } }
  const act = i.action?.id
  return {
    name: 'proposal',
    params: { owner, name, id: i.proposalId },
    query: act && act !== 'open' ? { do: act } : {},
  }
}
const itemLink = (i: AttentionItem): RouteLocationRaw =>
  i.proposalId
    ? { name: 'proposal', params: { ...split(i.repo), id: i.proposalId } }
    : { name: 'agents', params: split(i.repo) }
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <h1>Home</h1>
        <p class="muted">Review changes, unblock work, and keep agents moving.</p>
      </div>
      <Button v-if="canStart" to="/import">Import from GitHub / GitLab</Button>
      <Button
        v-if="canStart"
        variant="primary"
        icon="plus"
        to="/repos/new"
        data-testid="home-new-repo"
        >Start a repository</Button
      >
    </div>
    <ErrorState
      v-if="error && !repos.length"
      what="your home page"
      :error="error"
      @retry="reload"
    />
    <div v-else-if="loading && !settled" class="loading" aria-busy="true">
      <Skeleton :height="22" width="30%" /><Skeleton :height="90" /><Skeleton :height="90" />
    </div>
    <template v-else>
      <div class="toolbar" role="search" aria-label="Find work">
        <TextField
          :model-value="query"
          label="Search work"
          hide-label
          type="search"
          placeholder="Search changes, agents, repositories"
          @update:model-value="(value) => updateFilters({ q: value })"
          ><template #prefix><Icon name="search" :size="16" /></template
        ></TextField>
        <SelectField
          :model-value="repository"
          label="Repository"
          hide-label
          :options="repositoryOptions"
          @update:model-value="(value) => updateFilters({ repo: value })"
        />
        <Button icon="refresh" :loading="loading" data-testid="home-refresh" @click="reload"
          >Refresh</Button
        >
      </div>
      <p v-if="lastUpdated" class="refresh-note">
        <LiveIndicator v-if="repository" :status="liveStatus" />
        Last refreshed {{ formatTime(lastUpdated) }}.
        <template v-if="liveStatus === 'live'"
          >Live updates for {{ repository }}; a check every 15 seconds keeps the summary
          current.</template
        >
        <template v-else-if="repository"
          >Live updates {{ liveStatus === 'connecting' ? 'are connecting' : 'are unavailable' }}.
          Checks every 15 seconds while visible.</template
        >
        <template v-else
          >Checks every 15 seconds while visible. Select a repository for live updates.</template
        >
      </p>
      <Alert
        v-if="incomplete"
        tone="warn"
        title="This summary is incomplete"
        live="status"
        data-testid="attention-incomplete"
      >
        <template v-if="error">The last refresh failed. Previously loaded work is shown.</template>
        <template v-else-if="unreadable"
          >Some work could not be read or exceeds the per-repository summary limit.</template
        >
        <template v-if="skipped">
          Work from {{ repos.length - skipped }} of {{ repos.length }} repositories is loaded.
          Choose a repository above to see its work, or load more repositories.
          <Button :loading="loading" @click="loadMore">Load more repositories</Button>
        </template>
        <template v-else> Open the repository to see all work, or refresh to try again.</template>
      </Alert>
      <div class="workspace">
        <aside class="rail" aria-label="Work navigation">
          <nav class="filters" aria-label="Filter work">
            <button
              type="button"
              :class="{ selected: !kind }"
              :aria-pressed="!kind"
              @click="updateFilters({ kind: '' })"
            >
              <span>All work</span><span class="count">{{ scoped.length }}</span>
            </button>
            <button
              v-for="category in ATTENTION_ORDER"
              :key="category"
              type="button"
              :class="{ selected: kind === category }"
              :aria-pressed="kind === category"
              @click="updateFilters({ kind: category })"
            >
              <span>{{ ATTENTION_TITLE[category] }}</span
              ><span class="count">{{ counts.get(category) ?? 0 }}</span>
            </button>
          </nav>
          <section v-if="repos.length" class="repo-nav" aria-labelledby="home-repos-title">
            <h2 id="home-repos-title">Your repositories</h2>
            <RouterLink
              v-for="r in repos.slice(0, 8)"
              :key="r.name"
              :to="{ name: 'board', params: split(r.name) }"
              class="repo-link mono"
              >{{ r.name }}</RouterLink
            >
            <RouterLink to="/repos" class="all-repos"
              >All repositories <Icon name="arrow-right" :size="13"
            /></RouterLink>
          </section>
        </aside>
        <div class="work">
          <p class="summary" role="status" data-testid="attention-summary">
            {{
              filtering
                ? `${visible.length} of ${total} items shown`
                : incomplete
                  ? `${total} items in loaded repositories. Other work may still need your attention.`
                  : `${plural(total, 'thing needs', 'things need')} your attention.`
            }}
          </p>
          <EmptyState
            v-if="!repos.length"
            icon="database"
            title="Nothing to look at yet"
            :description="
              canStart
                ? 'Start with a repository. Choose a name and the rules for your team.'
                : 'You can see a repository once an administrator gives you a role on it. Ask them to add you.'
            "
            ><Button v-if="canStart" variant="primary" to="/repos/new"
              >Start a repository</Button
            ></EmptyState
          >
          <EmptyState
            v-else-if="!visible.length && filtering"
            icon="search"
            title="No work matches these filters"
            description="Try another search or show all work."
            ><Button @click="clearFilters">Clear filters</Button></EmptyState
          >
          <EmptyState
            v-else-if="!total && !incomplete"
            icon="check"
            title="Nothing needs you right now"
            description="No change is blocked, nothing waits for your review and no agent has gone quiet."
            data-testid="attention-clear"
            ><Button v-if="first" icon="bot" :to="{ name: 'agents', params: split(first.name) }"
              >Give an agent work</Button
            ><Button to="/repos">Browse repositories</Button></EmptyState
          >
          <EmptyState
            v-else-if="!total"
            icon="alert"
            title="We could not check all your work"
            description="The summary is incomplete. Refresh or open a repository to check its status."
            ><Button @click="reload">Try again</Button></EmptyState
          >
          <section
            v-for="g in groups"
            :key="g.kind"
            class="group"
            :aria-labelledby="`group-${g.kind}`"
            :data-testid="`attention-${g.kind}`"
          >
            <header class="group-head">
              <div class="group-title">
                <h2 :id="`group-${g.kind}`">{{ ATTENTION_TITLE[g.kind] }}</h2>
                <span class="count">{{ g.items.length }}</span>
              </div>
              <p>{{ ATTENTION_HELP[g.kind] }}</p>
            </header>
            <ul class="list" role="list">
              <li v-for="i in g.items" :key="i.key" class="row" data-testid="attention-item">
                <Icon
                  :name="
                    g.kind === 'ready'
                      ? 'check'
                      : g.kind === 'review'
                        ? 'proposal'
                        : g.kind === 'quiet'
                          ? 'clock'
                          : 'alert'
                  "
                  :size="18"
                  class="status-icon"
                  :class="`status-${g.kind}`"
                />
                <div class="item-main">
                  <RouterLink :to="itemLink(i)" class="title">{{ i.title }}</RouterLink>
                  <p class="reason">
                    {{ i.reason }}
                    <span v-if="i.more > 0" class="faint">(and {{ i.more }} more)</span>
                  </p>
                  <p class="meta">
                    <RouterLink :to="{ name: 'board', params: split(i.repo) }" class="mono">{{
                      i.repo
                    }}</RouterLink
                    ><span v-if="i.who"
                      ><Icon :name="i.byAgent ? 'bot' : 'user'" :size="12" />{{ i.who }}</span
                    ><span v-if="i.ageMs >= 60_000">{{ formatDuration(i.ageMs) }}</span>
                  </p>
                </div>
                <Button
                  v-if="i.action"
                  :variant="g.kind === 'ready' || g.kind === 'review' ? 'primary' : 'secondary'"
                  size="sm"
                  :to="target(i)"
                  :data-testid="`attention-action-${i.action.id}`"
                  >{{ i.action.label }}</Button
                >
              </li>
            </ul>
          </section>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-5);
  margin: 0 auto;
}
.head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  align-items: center;
  gap: var(--s-3);
}
.head h1 {
  text-wrap: balance;
  margin-bottom: var(--s-2);
}
.head .muted {
  font-size: var(--fs-md);
}
.toolbar {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(180px, 280px) auto;
  gap: var(--s-3);
  align-items: end;
  padding-bottom: var(--s-4);
  border-bottom: 1px solid var(--border);
}
.workspace {
  display: grid;
  grid-template-columns: 208px minmax(0, 1fr);
  gap: var(--s-6);
  align-items: start;
}
.rail {
  min-width: 0;
  position: sticky;
  top: calc(var(--header-h) + var(--s-5));
}
.filters {
  display: flex;
  flex-direction: column;
  gap: var(--s-1);
}
.filters button {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-2);
  width: 100%;
  min-height: 44px;
  padding: var(--s-3);
  border: 0;
  border-radius: var(--r-sm);
  background: transparent;
  color: var(--text-2);
  text-align: left;
  font-size: var(--fs-sm);
  white-space: nowrap;
}
.filters button:hover {
  background: var(--surface-3);
  color: var(--text);
}
.filters .selected {
  background: var(--accent-soft);
  color: var(--accent-text);
  font-weight: 600;
}
.count {
  font-variant-numeric: tabular-nums;
  color: var(--text-3);
  font-size: var(--fs-sm);
  font-weight: 400;
}
.selected .count {
  color: inherit;
}
.repo-nav {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  padding: var(--s-5) var(--s-3);
  margin-top: var(--s-4);
  border-top: 1px solid var(--border);
}
.repo-nav h2 {
  color: var(--text-2);
  font-size: var(--fs-sm);
  font-weight: 600;
}
.repo-link {
  overflow-wrap: anywhere;
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.all-repos {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-sm);
}
.work {
  min-width: 0;
  background: var(--surface);
  padding: var(--s-5);
  border-radius: var(--r-md);
}
.summary {
  color: var(--text-2);
  font-size: var(--fs-sm);
  margin-bottom: var(--s-5);
}
.group {
  margin-bottom: var(--s-6);
}
.group:last-child {
  margin-bottom: 0;
}
.group-head {
  padding-bottom: var(--s-3);
}
.group-head h2 {
  text-wrap: balance;
  display: flex;
  align-items: center;
  gap: var(--s-3);
  font-size: var(--fs-xl);
  margin-bottom: var(--s-2);
  overflow-wrap: break-word;
  min-width: 0;
}
.group-title {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}
.group-head .count {
  color: inherit;
}
.group-head p {
  color: var(--text-3);
  font-size: var(--fs-sm);
}
.list {
  list-style: none;
  padding: 0;
  margin: 0;
}
.row {
  display: flex;
  align-items: center;
  gap: var(--s-4);
  padding: var(--s-4) 0;
  border-top: 1px solid var(--border);
}
.row:last-child {
  border-bottom: 1px solid var(--border);
}
.status-icon {
  color: var(--text-3);
  flex: none;
}
.status-failed {
  color: var(--bad);
}
.status-blocked,
.status-quiet {
  color: var(--warn);
}
.status-ready {
  color: var(--ok);
}
.item-main {
  min-width: 0;
  flex: 1;
}
.title {
  font-size: var(--fs-lg);
  color: var(--text);
  font-weight: 600;
  overflow-wrap: anywhere;
}
.reason {
  color: var(--text-2);
  font-size: var(--fs-sm);
  margin-top: var(--s-1);
  overflow-wrap: anywhere;
}
.meta {
  display: flex;
  align-items: center;
  gap: var(--s-4);
  flex-wrap: wrap;
  margin-top: var(--s-2);
  color: var(--text-3);
  font-size: var(--fs-xs);
}
.meta a {
  color: var(--text-3);
  overflow-wrap: anywhere;
  max-width: 100%;
}
.meta span {
  min-width: 0;
  overflow-wrap: anywhere;
  display: inline-flex;
  gap: var(--s-1);
  align-items: center;
}
.refresh-note {
  font-size: var(--fs-xs);
  color: var(--text-3);
  overflow-wrap: anywhere;
}
.loading {
  display: grid;
  gap: var(--s-4);
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
  .workspace {
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-4);
  }
  .rail {
    position: static;
  }
  .filters {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
  .filters button {
    white-space: normal;
  }
  .repo-nav {
    display: none;
  }
}
@media (max-width: 600px) {
  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .head > div {
    width: 100%;
  }
  .toolbar {
    grid-template-columns: minmax(0, 1fr) auto;
  }
  .toolbar > :first-child {
    grid-column: 1 / -1;
  }
  .filters {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .work {
    padding: var(--s-4);
  }
  .row {
    flex-wrap: wrap;
    gap: var(--s-3);
  }
  .item-main {
    flex-basis: calc(100% - 40px);
  }
  .row > .btn {
    margin-left: var(--s-6);
  }
  .meta {
    font-size: var(--fs-sm);
    gap: var(--s-3);
  }
}
</style>
