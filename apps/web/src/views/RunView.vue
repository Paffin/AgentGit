<script setup lang="ts">
import { computed, onBeforeUnmount } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { useAuth } from '@/stores/auth'
import { usePrefs } from '@/stores/prefs'
import { useRepoName } from '@/composables/useRepoName'
import { useResource } from '@/composables/useResource'
import { usePageTitle } from '@/composables/usePageTitle'
import { useLiveReload } from '@/composables/useLiveReload'
import { useServerNow } from '@/composables/useNow'
import { runHealth } from '@/lib/runs'
import { formatDuration } from '@/lib/format'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import CopyButton from '@/ui/CopyButton.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{ id: string }>()
const { repo, routeParams } = useRepoName()
const auth = useAuth(),
  prefs = usePrefs(),
  now = useServerNow()
const supported = computed(() => !!auth.capabilities?.features.includes('agents.run_lookup'))
const scope = [repo, () => props.id, () => auth.token, supported]
const run = useResource(
  (signal) =>
    api.agents.run(repo.value, props.id, { signal, token: auth.token, quietUnauthenticated: true }),
  { watch: scope, enabled: () => supported.value },
)
const task = useResource(
  (signal) =>
    api.agents.task(repo.value, run.data.value?.task_id || '', {
      signal,
      token: auth.token,
      quietUnauthenticated: true,
    }),
  {
    watch: [repo, () => run.data.value?.task_id, () => auth.token],
    enabled: () => !!run.data.value?.task_id,
  },
)
const leases = useResource(
  (signal) =>
    api.agents.leases(repo.value, true, { signal, token: auth.token, quietUnauthenticated: true }),
  { watch: [...scope, () => run.data.value?.identity], enabled: () => !!run.data.value },
)
const reservations = computed(() =>
  (leases.data.value || []).filter((lease) => lease.holder === run.data.value?.identity),
)
const health = computed(() =>
  run.data.value ? runHealth(run.data.value, now.value, prefs.staleAfterMs) : null,
)
const duration = computed(() =>
  run.data.value
    ? formatDuration(
        Math.max(0, (run.data.value.finished_at_ms || now.value) - run.data.value.started_at_ms),
      )
    : '',
)
const permalink = computed(
  () =>
    `${window.location.origin}/repos/${repo.value.split('/').map(encodeURIComponent).join('/')}/runs/${encodeURIComponent(props.id)}`,
)
const changesLink = computed(() => ({
  name: 'proposals',
  params: routeParams.value,
  query: { state: 'all', author: run.data.value?.identity || '' },
}))
usePageTitle(() => `${run.data.value?.agent || 'Agent'} run · ${props.id} · ${repo.value}`)
async function refresh() {
  await Promise.all([run.reload(), task.reload(), leases.reload()])
}
useLiveReload(['run', 'task', 'lease', 'proposal'], refresh)
const heartbeatPoll = setInterval(() => {
  if (!document.hidden && run.data.value?.status === 'running') void run.reload()
}, 10000)
onBeforeUnmount(() => clearInterval(heartbeatPoll))
</script>

<template>
  <div class="run-page">
    <RouterLink :to="{ name: 'agents', params: routeParams, query: { tab: 'runs' } }"
      >← Agent sessions</RouterLink
    >
    <Alert v-if="!supported" tone="warn" title="Server update required"
      >This server does not support permanent run pages.</Alert
    >
    <ErrorState v-else-if="run.error.value" :error="run.error.value" @retry="refresh" />
    <p v-else-if="!run.data.value" role="status">Loading agent run…</p>
    <template v-else>
      <header class="heading">
        <div>
          <h1>Run · {{ run.data.value.agent }}</h1>
          <p class="run-id">{{ run.data.value.run_id }}</p>
        </div>
        <div class="actions">
          <Button @click="refresh">Refresh</Button
          ><CopyButton :text="permalink" label="Copy run link" with-label />
        </div>
      </header>
      <div class="status-line">
        <Badge
          :tone="
            run.data.value.status === 'failed' || run.data.value.status === 'abandoned'
              ? 'bad'
              : health?.liveness === 'stale'
                ? 'warn'
                : 'neutral'
          "
          >{{ run.data.value.status }}</Badge
        >
        <span v-if="health?.liveness === 'stale'"
          >No heartbeat for {{ formatDuration(health.ageMs) }}</span
        >
        <span>{{ duration }}</span>
      </div>
      <p v-if="run.data.value.status === 'completed'" class="muted">
        Completed describes this session. Its changes still need their own validation, review and
        integration.
      </p>
      <Alert v-if="health?.liveness === 'stale'" tone="warn"
        >The server has not heard from this running session recently. This does not confirm that its
        external process stopped.</Alert
      >
      <dl class="facts">
        <div>
          <dt>Session identity</dt>
          <dd><IdentityLabel :identity="run.data.value.identity" /></dd>
        </div>
        <div>
          <dt>Started</dt>
          <dd><RelativeTime :ms="run.data.value.started_at_ms" /></dd>
        </div>
        <div>
          <dt>Last heartbeat</dt>
          <dd>
            <RelativeTime :ms="run.data.value.last_heartbeat_ms || run.data.value.started_at_ms" />
          </dd>
        </div>
        <div v-if="run.data.value.finished_at_ms">
          <dt>Finished</dt>
          <dd><RelativeTime :ms="run.data.value.finished_at_ms" /></dd>
        </div>
      </dl>
      <section aria-labelledby="summary-heading">
        <h2 id="summary-heading">Session summary</h2>
        <p class="summary">
          {{ run.data.value.summary || 'The agent has not provided a summary.' }}
        </p>
      </section>
      <section aria-labelledby="task-heading">
        <h2 id="task-heading">Assigned task</h2>
        <p v-if="!run.data.value.task_id" class="muted">This session was started without a task.</p>
        <ErrorState v-else-if="task.error.value" :error="task.error.value" @retry="task.reload()" />
        <template v-else>
          <RouterLink
            :to="{ name: 'task', params: { ...routeParams, id: run.data.value.task_id } }"
            >{{ task.data.value?.title || run.data.value.task_id }}</RouterLink
          >
          <p v-if="task.data.value?.description" class="summary">
            {{ task.data.value.description }}
          </p>
          <p class="muted">
            Open the task to inspect dependencies and available completion or cancellation actions.
            Connected orchestrators stop managed sessions after they observe task cancellation.
          </p>
        </template>
      </section>
      <section aria-labelledby="changes-heading">
        <h2 id="changes-heading">Changes from this session</h2>
        <RouterLink :to="changesLink">View this run's change requests</RouterLink>
        <p class="muted">
          Filtered by this exact session identity, including open, closed and integrated requests.
        </p>
      </section>
      <section aria-labelledby="reservations-heading">
        <h2 id="reservations-heading">Current file reservations</h2>
        <ErrorState
          v-if="leases.error.value"
          :error="leases.error.value"
          @retry="leases.reload()"
        />
        <ul v-else-if="reservations.length">
          <li v-for="lease in reservations" :key="lease.id">
            <code>{{ lease.scope }}</code> · {{ lease.exclusive ? 'exclusive' : 'shared' }} ·
            expires <RelativeTime :ms="lease.expires_at_ms" />
          </li>
        </ul>
        <p v-else class="muted">No active reservations held by this session.</p>
      </section>
    </template>
  </div>
</template>

<style scoped>
.run-page {
  display: grid;
  gap: var(--s-4);
  max-width: 1100px;
}
.heading,
.actions,
.status-line {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.heading > div {
  min-width: 0;
  overflow-wrap: anywhere;
}
.heading {
  justify-content: space-between;
  align-items: start;
}
.run-id {
  font-family: var(--font-mono);
  color: var(--text-2);
  overflow-wrap: anywhere;
}
.facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-4);
}
dt,
.muted {
  color: var(--text-2);
}
dd {
  margin: var(--s-1) 0 0;
  overflow-wrap: anywhere;
}
section {
  border-top: 1px solid var(--border);
  padding-top: var(--s-4);
}
.summary {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
</style>
