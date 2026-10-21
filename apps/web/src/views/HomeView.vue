<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { RouteLocationRaw } from 'vue-router'
import { useAttention } from '@/composables/useAttention'
import { usePageTitle } from '@/composables/usePageTitle'
import { ATTENTION_HELP, ATTENTION_TITLE, groupAttention } from '@/lib/attention'
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
import type { IconName } from '@/ui/icons'

const auth = useAuth()
usePageTitle(() => 'Home')
const { repos, items, loading, settled, error, skipped, unreadable, reload } = useAttention()

const groups = computed(() => groupAttention(items.value))
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
        <h1>Home</h1>
        <p class="muted">
          What needs you right now, across the repositories you can read.
          <span class="faint">Refreshed every few seconds while this page is open.</span>
        </p>
      </div>
      <Button
        v-if="canStart"
        variant="primary"
        icon="plus"
        to="/repos/new"
        data-testid="home-new-repo"
        >Start a repository</Button
      >
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
      <p v-if="total" class="summary" role="status" data-testid="attention-summary">
        {{ plural(total, 'thing needs', 'things need') }} your attention.
      </p>

      <Card v-if="!repos.length" flush>
        <EmptyState
          icon="database"
          title="Nothing to look at yet"
          :description="
            canStart
              ? 'Start with a repository. It takes a minute: a name, and how strict the rules should be.'
              : 'You can see a repository once an administrator gives you a role on it. Ask them to add you.'
          "
        >
          <Button v-if="canStart" variant="primary" icon="plus" to="/repos/new"
            >Start a repository</Button
          >
        </EmptyState>
      </Card>

      <Card v-else-if="!total" flush data-testid="attention-clear">
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

      <Card
        v-for="g in groups"
        :key="g.kind"
        :title="ATTENTION_TITLE[g.kind]"
        :subtitle="ATTENTION_HELP[g.kind]"
        :icon="ICON[g.kind]"
        :tone="g.kind === 'failed' ? 'bad' : g.kind === 'blocked' ? 'warn' : 'default'"
        flush
        :data-testid="`attention-${g.kind}`"
      >
        <template #actions
          ><Badge :tone="TONE[g.kind]">{{ g.items.length }}</Badge></template
        >
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
      </Card>

      <p v-if="unreadable" class="faint note" role="status">
        {{ plural(unreadable, 'repository') }} could not be read this time; the rest is shown.
      </p>
      <p v-if="skipped" class="faint note">
        This summary reads the 30 newest repositories.
        <RouterLink to="/repos">See all repositories</RouterLink>
      </p>

      <Card v-if="repos.length" title="Your repositories" icon="database" flush>
        <template #actions><RouterLink to="/repos">All repositories</RouterLink></template>
        <ul class="repos" role="list">
          <li v-for="r in repos.slice(0, 8)" :key="r.name">
            <RouterLink :to="{ name: 'board', params: split(r.name) }" class="mono repo">{{
              r.name
            }}</RouterLink>
            <span class="faint desc">{{ r.description }}</span>
          </li>
        </ul>
      </Card>
    </template>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  max-width: 1100px;
}
.head {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.loading {
  display: grid;
  gap: var(--s-3);
}
.summary {
  font-weight: 560;
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
  padding: var(--s-3) var(--s-4);
  border-top: 1px solid var(--border);
}
.row:first-child {
  border-top: 0;
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
  margin-top: 2px;
  overflow-wrap: anywhere;
}
.meta {
  display: flex;
  gap: var(--s-3);
  align-items: center;
  flex-wrap: wrap;
  font-size: var(--fs-xs);
  margin-top: var(--s-1);
}
.meta span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.note {
  font-size: var(--fs-sm);
}
.repos li {
  display: flex;
  gap: var(--s-3);
  align-items: baseline;
  padding: var(--s-2) var(--s-4);
  border-top: 1px solid var(--border);
  min-width: 0;
}
.repos li:first-child {
  border-top: 0;
}
.repo {
  font-weight: 600;
  flex: none;
}
.desc {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}
@media (max-width: 640px) {
  .row {
    flex-direction: column;
    align-items: stretch;
  }
}
</style>
