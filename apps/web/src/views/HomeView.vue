<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import type { RouteLocationRaw } from 'vue-router'
import { useAttention } from '@/composables/useAttention'
import { usePageTitle } from '@/composables/usePageTitle'
import { ATTENTION_HELP, ATTENTION_ORDER, ATTENTION_TITLE, groupAttention } from '@/lib/attention'
import type { AttentionItem, AttentionKind } from '@/lib/attention'
import { formatDuration, plural } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextField from '@/ui/TextField.vue'
import SelectField from '@/ui/SelectField.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import type { IconName } from '@/ui/icons'

const auth = useAuth()
usePageTitle(() => 'Home')
const {
  repos,
  items,
  loading,
  settled,
  refreshing,
  lastUpdated,
  error,
  skipped,
  unreadable,
  reload,
} = useAttention()

const query = ref('')
const repository = ref('')
const kind = ref<AttentionKind | 'all'>('all')
const LABEL: Record<AttentionKind, string> = {
  failed: 'Failures',
  review: 'Reviews',
  blocked: 'Blocked',
  ready: 'Ready to land',
  quiet: 'Silent agents',
}
const counts = computed(() =>
  Object.fromEntries(
    ATTENTION_ORDER.map((k) => [k, items.value.filter((i) => i.kind === k).length]),
  ),
)
const visible = computed(() => {
  const terms = query.value.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  return items.value.filter(
    (i) =>
      (kind.value === 'all' || i.kind === kind.value) &&
      (!repository.value || i.repo === repository.value) &&
      terms.every((term) =>
        `${i.title} ${i.reason} ${i.repo} ${i.who}`.toLocaleLowerCase().includes(term),
      ),
  )
})
const groups = computed(() => groupAttention(visible.value))
const repoOptions = computed(() => [
  { value: '', label: 'All repositories' },
  ...repos.value.map((r) => ({ value: r.name, label: r.name })),
])
const filtered = computed(() => !!query.value.trim() || !!repository.value || kind.value !== 'all')
function resetFilters() {
  query.value = ''
  repository.value = ''
  kind.value = 'all'
}
const repoCount = (name: string) => items.value.filter((i) => i.repo === name).length
const ICON: Record<AttentionKind, IconName> = {
  failed: 'x',
  review: 'comment',
  blocked: 'alert',
  ready: 'merge',
  quiet: 'clock',
}
const TONE: Record<AttentionKind, 'bad' | 'warn' | 'ok' | 'info'> = {
  failed: 'bad',
  review: 'info',
  blocked: 'warn',
  ready: 'ok',
  quiet: 'warn',
}

const split = (repo: string) => {
  const [owner = '', name = ''] = repo.split('/')
  return { owner, name }
}

/** Where the call to action goes; `do` makes the change-request page open the dialog or tab that does it. */
function target(i: AttentionItem): RouteLocationRaw {
  const { owner, name } = split(i.repo)
  if (i.proposalId) {
    const act = i.action?.id
    return {
      name: 'proposal',
      params: { owner, name, id: i.proposalId },
      query: act && act !== 'open' ? { do: act } : {},
    }
  }
  return { name: 'agents', params: { owner, name } }
}
const itemLink = (i: AttentionItem): RouteLocationRaw =>
  i.proposalId
    ? { name: 'proposal', params: { ...split(i.repo), id: i.proposalId } }
    : { name: 'agents', params: split(i.repo) }

const total = computed(() => items.value.length)
const canStart = computed(() => auth.isAdminAnywhere)
const first = computed(() => repos.value[0])
</script>

<template>
  <div class="page">
    <div class="head">
      <div>
        <p class="eyebrow">Agent operations</p>
        <h1>Home</h1>
        <p class="muted">Review, unblock, and land work across your repositories.</p>
      </div>
      <div class="head-actions">
        <Button variant="ghost" icon="refresh" :loading="refreshing" @click="reload"
          >Refresh</Button
        >
        <Button
          v-if="canStart"
          variant="primary"
          icon="plus"
          to="/repos/new"
          data-testid="home-new-repo"
          >Start a repository</Button
        >
      </div>
    </div>

    <ErrorState v-if="error && !settled" what="your home page" :error="error" @retry="reload" />
    <ErrorState
      v-else-if="error && !items.length"
      what="your home page"
      :error="error"
      @retry="reload"
    />

    <div v-else-if="loading && !settled" class="loading" aria-busy="true">
      <Skeleton :height="22" width="30%" />
      <Skeleton :height="90" radius="10px" />
      <Skeleton :height="90" radius="10px" />
    </div>

    <template v-else>
      <div class="status-line">
        <p v-if="total" class="summary" role="status" data-testid="attention-summary">
          {{ plural(total, 'thing needs', 'things need') }} your attention.
        </p>
        <span v-if="lastUpdated" class="freshness"
          >Updated <RelativeTime :ms="lastUpdated" /> · checks every 15s</span
        >
      </div>

      <div v-if="repos.length" class="workspace">
        <div class="work-column">
          <nav v-if="total" class="filters" aria-label="Filter attention">
            <button type="button" :aria-pressed="kind === 'all'" @click="kind = 'all'">
              All work <span>{{ total }}</span>
            </button>
            <button
              v-for="k in ATTENTION_ORDER"
              :key="k"
              type="button"
              :aria-pressed="kind === k"
              :data-kind="k"
              @click="kind = k"
            >
              {{ LABEL[k] }} <span>{{ counts[k] }}</span>
            </button>
          </nav>
          <div v-if="total" class="search-row">
            <TextField
              v-model="query"
              type="search"
              label="Search work"
              hide-label
              placeholder="Search changes, agents, or blockers…"
              ><template #prefix><Icon name="search" :size="16" /></template
            ></TextField>
            <SelectField
              v-model="repository"
              label="Filter by repository"
              hide-label
              :options="repoOptions"
            />
          </div>
          <div v-if="filtered" class="filter-result">
            <span>{{ visible.length }} of {{ total }} items</span>
            <button type="button" @click="resetFilters">Clear filters</button>
          </div>

          <Card v-if="total && !visible.length" flush data-testid="attention-no-results">
            <EmptyState
              icon="search"
              title="No work matches these filters"
              description="Try another name, repository, or state. Your work is still here."
              ><Button @click="resetFilters">Clear filters</Button></EmptyState
            >
          </Card>

          <Card v-if="!total" flush data-testid="attention-clear">
            <EmptyState
              icon="check"
              title="Nothing needs you right now"
              description="No change is blocked, nothing waits for your review and no agent has gone quiet. When that changes it shows up here."
            >
              <Button v-if="first" icon="bot" :to="{ name: 'agents', params: split(first.name) }"
                >Give an agent work</Button
              >
              <Button icon="database" to="/repos">Browse repositories</Button>
            </EmptyState>
          </Card>

          <div v-if="groups.length" class="queue" :aria-busy="refreshing">
            <section
              v-for="g in groups"
              :key="g.kind"
              class="work-group"
              :data-kind="g.kind"
              :aria-labelledby="`group-${g.kind}`"
              :data-testid="`attention-${g.kind}`"
            >
              <header class="group-head">
                <div>
                  <h2 :id="`group-${g.kind}`">
                    <Icon :name="ICON[g.kind]" :size="15" />{{ ATTENTION_TITLE[g.kind]
                    }}<Badge :tone="TONE[g.kind]">{{ g.items.length }}</Badge>
                  </h2>
                  <p>{{ ATTENTION_HELP[g.kind] }}</p>
                </div>
              </header>
              <ul class="list" role="list">
                <li v-for="i in g.items" :key="i.key" class="row" :data-testid="`attention-item`">
                  <div class="main">
                    <RouterLink :to="itemLink(i)" class="title">{{ i.title }}</RouterLink>
                    <p class="reason">
                      {{ i.reason }}
                      <span v-if="i.more > 0" class="faint">(and {{ i.more }} more)</span>
                    </p>
                    <p class="meta faint">
                      <Badge mono size="sm">{{ i.repo }}</Badge>
                      <span v-if="i.who">
                        <Icon :name="i.byAgent ? 'bot' : 'user'" :size="12" />
                        {{ i.who }}<template v-if="i.byAgent"> (agent, waiting)</template>
                      </span>
                      <span v-if="i.ageMs >= 60_000">for {{ formatDuration(i.ageMs) }}</span>
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

          <p v-if="unreadable" class="faint note" role="status">
            {{ plural(unreadable, 'repository') }} could not be read this time; the rest is shown.
          </p>
          <p v-if="skipped" class="faint note">
            This summary reads the 30 newest repositories.
            <RouterLink to="/repos">See all repositories</RouterLink>
          </p>
        </div>
        <aside class="repo-rail" aria-label="Your repositories">
          <header>
            <h2>Your repositories</h2>
            <RouterLink to="/repos">View all</RouterLink>
          </header>
          <p class="rail-help">Open a workspace to see its agents, changes, and rules.</p>
          <ul class="repos" role="list">
            <li v-for="r in repos.slice(0, 8)" :key="r.name">
              <div>
                <RouterLink :to="{ name: 'board', params: split(r.name) }" class="mono repo">{{
                  r.name
                }}</RouterLink>
                <span class="faint desc">{{ r.description }}</span>
              </div>
              <span
                v-if="repoCount(r.name)"
                class="repo-count"
                :aria-label="`${repoCount(r.name)} items need attention`"
                >{{ repoCount(r.name) }}</span
              >
            </li>
          </ul>
          <p class="rail-foot">
            {{ repos.length }} {{ repos.length === 1 ? 'repository' : 'repositories' }} loaded
          </p>
        </aside>
      </div>
      <Card v-else flush>
        <EmptyState
          icon="database"
          title="Nothing to look at yet"
          :description="
            canStart
              ? 'Start with a repository. It takes a minute: a name, and how strict the rules should be.'
              : 'You can see a repository once an administrator gives you a role on it. Ask them to add you.'
          "
          ><Button v-if="canStart" variant="primary" icon="plus" to="/repos/new"
            >Start a repository</Button
          ></EmptyState
        >
      </Card>
    </template>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-5);
}
.head,
.head-actions,
.status-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.head-actions {
  justify-content: flex-end;
}
.eyebrow {
  font-family: var(--font-mono);
  font-size: var(--fs-xs);
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--text-3);
  margin-bottom: var(--s-2);
}
.head h1 {
  font-size: calc(var(--fs-2xl) * 1.25);
  margin-bottom: var(--s-2);
}
.status-line {
  font-size: var(--fs-sm);
}
.summary {
  font-weight: 600;
}
.freshness {
  color: var(--text-3);
  font-size: var(--fs-xs);
}
.workspace {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 280px;
  gap: var(--s-5);
  align-items: start;
}
.work-column {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.filters {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-1);
  padding: var(--s-1);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  background: var(--surface-2);
}
.filters button {
  display: inline-flex;
  gap: var(--s-2);
  align-items: center;
  justify-content: center;
  min-height: max(36px, var(--touch-min));
  padding: var(--s-2) var(--s-3);
  border: 0;
  border-radius: var(--r-sm);
  background: transparent;
  color: var(--text-2);
  font-weight: 600;
  font-size: var(--fs-sm);
}
.filters button:hover {
  background: var(--surface);
  color: var(--text);
}
.filters button[aria-pressed='true'] {
  background: var(--text);
  color: var(--surface);
}
.filters button span {
  font: 500 var(--fs-xs) var(--font-mono);
  opacity: 0.8;
}
.search-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 210px;
  gap: var(--s-3);
}
.filter-result {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  color: var(--text-3);
  font-size: var(--fs-sm);
}
.filter-result button {
  border: 0;
  background: transparent;
  color: var(--accent-text);
  text-decoration: underline;
  padding: var(--s-1);
}
.queue {
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  background: var(--surface);
  overflow: hidden;
}
.work-group + .work-group {
  border-top: 1px solid var(--border);
}
.group-head {
  background: var(--surface-2);
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
}
.group-head h2 {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-md);
}
.group-head p {
  color: var(--text-3);
  font-size: var(--fs-xs);
  margin-top: var(--s-1);
}
.work-group[data-kind='failed'] .group-head :deep(.icon) {
  color: var(--bad);
}
.work-group[data-kind='blocked'] .group-head :deep(.icon) {
  color: var(--warn);
}
.work-group[data-kind='ready'] .group-head :deep(.icon) {
  color: var(--ok);
}
.list,
.repos {
  list-style: none;
  margin: 0;
  padding: 0;
}
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-4);
  padding: var(--s-4);
}
.row + .row {
  border-top: 1px solid var(--border);
}
.row:hover {
  background: var(--code-bg);
}
.main {
  min-width: 0;
}
.title {
  font-weight: 600;
  color: var(--text);
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
  gap: var(--s-3);
  align-items: center;
  flex-wrap: wrap;
  font-size: var(--fs-xs);
  margin-top: var(--s-2);
}
.meta span {
  display: inline-flex;
  align-items: center;
  gap: var(--s-1);
}
.note {
  font-size: var(--fs-sm);
}
.repo-rail {
  min-width: 0;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  position: sticky;
  top: calc(var(--header-h) + var(--s-4));
}
.repo-rail header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
  padding: var(--s-4) var(--s-4) var(--s-2);
}
.repo-rail h2 {
  font-size: var(--fs-md);
}
.repo-rail header a {
  font-size: var(--fs-xs);
}
.rail-help {
  padding: 0 var(--s-4) var(--s-4);
  color: var(--text-3);
  font-size: var(--fs-xs);
}
.repos li {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
  padding: var(--s-3) var(--s-4);
  border-top: 1px solid var(--border);
  min-width: 0;
}
.repos li > div {
  min-width: 0;
}
.repo {
  font-weight: 600;
  overflow-wrap: anywhere;
}
.desc {
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
  font-size: var(--fs-xs);
  margin-top: var(--s-1);
}
.repo-count {
  flex: none;
  display: grid;
  place-items: center;
  min-width: 24px;
  height: 24px;
  border-radius: var(--r-sm);
  background: var(--surface-2);
  font: 500 var(--fs-xs) var(--font-mono);
}
.rail-foot {
  border-top: 1px solid var(--border);
  padding: var(--s-3) var(--s-4);
  color: var(--text-3);
  font-size: var(--fs-xs);
}
.loading {
  display: grid;
  gap: var(--s-3);
}
@media (max-width: 1100px) {
  .workspace {
    grid-template-columns: minmax(0, 1fr);
  }
  .repo-rail {
    position: static;
  }
  .repos {
    display: grid;
    grid-template-columns: 1fr 1fr;
  }
}
@media (max-width: 640px) {
  .search-row,
  .repos {
    grid-template-columns: minmax(0, 1fr);
  }
  .row {
    flex-direction: column;
    align-items: stretch;
  }
  .row :deep(.btn) {
    align-self: flex-start;
  }
  .head-actions {
    justify-content: flex-start;
  }
  .filters button {
    flex: 1 0 auto;
  }
}
</style>
