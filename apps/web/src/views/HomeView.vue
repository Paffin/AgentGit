<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
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
import SelectField from '@/ui/SelectField.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextField from '@/ui/TextField.vue'

const auth = useAuth()
usePageTitle(() => 'Home')
const { repos, items, loading, settled, lastUpdated, error, skipped, unreadable, reload } =
  useAttention()
const query = ref('')
const repository = ref('')
const kind = ref<AttentionKind>()
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
  query.value = ''
  repository.value = ''
  kind.value = undefined
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
          v-model="query"
          label="Search work"
          hide-label
          type="search"
          placeholder="Search changes, agents, repositories"
          ><template #prefix><Icon name="search" :size="16" /></template
        ></TextField>
        <SelectField
          v-model="repository"
          label="Repository"
          hide-label
          :options="repositoryOptions"
        />
        <Button icon="refresh" :loading="loading" data-testid="home-refresh" @click="reload"
          >Refresh</Button
        >
      </div>
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
        <template v-if="skipped"> Only the 30 newest repositories are included.</template>
        Open the repository to see all work, or refresh to try again.
      </Alert>
      <div class="workspace">
        <aside class="rail" aria-label="Work navigation">
          <nav class="filters" aria-label="Filter work">
            <button
              type="button"
              :class="{ selected: !kind }"
              :aria-pressed="!kind"
              @click="kind = undefined"
            >
              <span>All work</span><span class="count">{{ scoped.length }}</span>
            </button>
            <button
              v-for="category in ATTENTION_ORDER"
              :key="category"
              type="button"
              :class="{ selected: kind === category }"
              :aria-pressed="kind === category"
              @click="kind = category"
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
              <h2 :id="`group-${g.kind}`">
                {{ ATTENTION_TITLE[g.kind] }} <span class="count">{{ g.items.length }}</span>
              </h2>
              <p>{{ ATTENTION_HELP[g.kind] }}</p>
            </header>
            <ul class="list" role="list">
              <li v-for="i in g.items" :key="i.key" class="row" data-testid="attention-item">
                <span class="status-dot" :class="`status-${g.kind}`" aria-hidden="true" />
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
          <p v-if="lastUpdated" class="refresh-note">
            Last refreshed {{ formatTime(lastUpdated) }}. Checks every 15 seconds while visible.
          </p>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.page {
  --rail-width: 220px;
  --rail-gap: 4rem;
  display: flex;
  flex-direction: column;
  gap: var(--s-5);
  max-width: none;
  margin: 0 auto;
}
.head {
  margin-left: calc(var(--rail-width) + var(--rail-gap));
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-4);
  flex-wrap: wrap;
  padding: var(--s-3) 0;
}
h1 {
  font-size: 2rem;
  letter-spacing: -0.035em;
  margin-bottom: var(--s-2);
}
.toolbar {
  margin-left: calc(var(--rail-width) + var(--rail-gap));
  display: grid;
  grid-template-columns: minmax(180px, 1fr) minmax(160px, 280px) auto;
  align-items: end;
  gap: var(--s-3);
}
.toolbar :deep(input),
.toolbar :deep(select),
.toolbar :deep(.btn) {
  min-height: max(40px, var(--touch-min));
}
.workspace {
  display: grid;
  grid-template-columns: var(--rail-width) minmax(0, 1fr);
  gap: var(--rail-gap);
}
.rail {
  min-width: 0;
}
.filters {
  display: flex;
  flex-direction: column;
  gap: 3px;
}
.filters button {
  display: flex;
  justify-content: space-between;
  gap: var(--s-2);
  align-items: center;
  width: 100%;
  min-height: 42px;
  padding: 10px 12px;
  background: transparent;
  border: 0;
  border-radius: var(--r-sm);
  color: var(--text-2);
  text-align: left;
  font-size: var(--fs-sm);
}
.filters button:hover {
  background: var(--surface-2);
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
  font-weight: 450;
}
.selected .count {
  color: inherit;
}
.repo-nav {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  padding: var(--s-5) 12px;
  margin-top: var(--s-5);
  border-top: 1px solid var(--border);
}
.repo-nav h2 {
  color: var(--text-3);
  font-size: var(--fs-xs);
  font-weight: 550;
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
  font-size: var(--fs-xs);
}
.work {
  min-width: 0;
}
.summary {
  color: var(--text-3);
  font-size: var(--fs-xs);
  margin-bottom: var(--s-5);
}
.group {
  margin-bottom: var(--s-6);
}
.group-head {
  padding-bottom: var(--s-3);
}
.group-head h2 {
  font-size: var(--fs-xl);
  display: flex;
  align-items: center;
  gap: var(--s-3);
  margin-bottom: 5px;
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
  padding: 18px 0;
  border-top: 1px solid var(--border);
}
.row:last-child {
  border-bottom: 1px solid var(--border);
}
.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--text-3);
  flex: none;
}
.status-failed {
  background: var(--bad);
}
.status-blocked,
.status-quiet {
  background: var(--warn);
}
.status-ready {
  background: var(--ok);
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
  margin-top: 4px;
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
  gap: 5px;
  align-items: center;
}
.refresh-note {
  font-size: var(--fs-xs);
  color: var(--text-3);
  padding-top: var(--s-2);
}
.loading {
  display: grid;
  gap: var(--s-4);
}
@media (max-width: 820px) {
  .head,
  .toolbar {
    margin-left: 0;
  }
  .workspace {
    grid-template-columns: 1fr;
    gap: var(--s-4);
  }
  .filters {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .repo-nav {
    display: none;
  }
  .toolbar {
    grid-template-columns: 1fr auto;
  }
  .toolbar > :first-child {
    grid-column: 1 / -1;
  }
}
@media (max-width: 480px) {
  h1 {
    font-size: 1.65rem;
  }
  .row {
    flex-wrap: wrap;
    gap: var(--s-3);
  }
  .item-main {
    flex-basis: calc(100% - 24px);
  }
  .row > :last-child {
    margin-left: 20px;
  }
  .head {
    align-items: flex-start;
  }
}
</style>
