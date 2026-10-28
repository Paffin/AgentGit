<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import type { Issue } from '@/api/types'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { useLiveReload } from '@/composables/useLiveReload'
import { useIssueDraft } from '@/composables/useIssueDraft'
import Button from '@/ui/Button.vue'
import Alert from '@/ui/Alert.vue'
import TextField from '@/ui/TextField.vue'
import TextArea from '@/ui/TextArea.vue'
import SelectField from '@/ui/SelectField.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import EmptyState from '@/ui/EmptyState.vue'
const { repo, routeParams } = useRepoName()
const auth = useAuth(),
  route = useRoute(),
  router = useRouter()
usePageTitle(() => `Issues · ${repo.value}`)
const supported = computed(() => !!auth.capabilities?.features.includes('issues.v1'))
const canWrite = computed(() => auth.can(repo.value, 'createIssue'))
const identity = computed(() => JSON.stringify([auth.identity, repo.value]))
const recovery = useIssueDraft(() => (auth.identity ? identity.value + ':new' : ''))
const { draft } = recovery
const creating = computed(() => route.query.new === '1')
const query = ref(typeof route.query.q === 'string' ? route.query.q : '')
const state = ref(typeof route.query.state === 'string' ? route.query.state : 'open')
const states = [
  { value: 'open', label: 'Open' },
  { value: 'closed', label: 'Closed' },
  { value: 'all', label: 'All issues' },
]
const rows = ref<Issue[]>([]),
  next = ref(''),
  busy = ref(false),
  saving = ref(false),
  error = ref('')
let serial = 0,
  controller: AbortController | undefined
async function load(append = false) {
  if (!supported.value) return
  controller?.abort()
  controller = new AbortController()
  const mine = ++serial
  busy.value = true
  error.value = ''
  try {
    const page = await api.issues.list(
      repo.value,
      {
        state: state.value === 'all' ? '' : state.value,
        query: query.value,
        page_size: 50,
        page_token: append ? next.value : undefined,
      },
      { signal: controller.signal, token: auth.token, quietUnauthenticated: true },
    )
    if (mine !== serial) return
    rows.value = append ? [...rows.value, ...page.issues] : page.issues
    next.value = page.next_page_token
  } catch (e) {
    if (mine === serial) error.value = describeError(e)
  } finally {
    if (mine === serial) busy.value = false
  }
}
function filters() {
  void router.replace({
    query: {
      ...route.query,
      q: query.value || undefined,
      state: state.value === 'open' ? undefined : state.value,
    },
  })
}
function showCreate(value: boolean) {
  void router.replace({ query: { ...route.query, new: value ? '1' : undefined } })
}
watch(
  [repo, () => auth.token, () => route.query.q, () => route.query.state, supported],
  () => {
    serial++
    controller?.abort()
    rows.value = []
    next.value = ''
    error.value = ''
    query.value = typeof route.query.q === 'string' ? route.query.q : ''
    state.value = states.some((x) => x.value === route.query.state)
      ? String(route.query.state)
      : 'open'
    void load()
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  serial++
  controller?.abort()
})
async function create() {
  if (saving.value || !draft.value.title.trim() || !canWrite.value) return
  const scope = identity.value
  const body = { title: draft.value.title, body: draft.value.body }
  const token = auth.token
  const idempotencyKey = draft.value.requestId
  saving.value = true
  error.value = ''
  try {
    const issue = await api.issues.create(repo.value, body, {
      token,
      idempotencyKey,
      quietUnauthenticated: true,
    })
    if (scope !== identity.value) return
    recovery.clear()
    showCreate(false)
    await router.push({ name: 'issue', params: { ...routeParams.value, number: issue.number } })
  } catch (e) {
    if (scope === identity.value) error.value = describeError(e)
  } finally {
    if (scope === identity.value) saving.value = false
  }
}
watch(identity, () => {
  saving.value = false
})
useLiveReload(['issue'], () => load())
</script>
<template>
  <div class="issues-page">
    <header class="heading">
      <div>
        <h1>Issues</h1>
        <p class="muted">Plan work, discuss it, and connect it to an agent task.</p>
      </div>
      <Button v-if="canWrite && supported && !creating" variant="primary" @click="showCreate(true)"
        >New issue</Button
      >
    </header>
    <Alert v-if="!supported" tone="warn" title="Server update required"
      >This server does not support the issue tracker yet.</Alert
    >
    <Alert v-if="error" tone="bad" live="alert" title="The request could not finish"
      >{{ error }} <Button size="sm" @click="load()">Reload issues</Button></Alert
    >
    <form v-if="creating && canWrite && supported" class="issue-form" @submit.prevent="create">
      <h2>New issue</h2>
      <TextField v-model="draft.title" label="Title" required :disabled="saving" />
      <TextArea
        v-model="draft.body"
        label="Description"
        :rows="7"
        :disabled="saving"
        hint="Explain the problem, expected result and useful context."
      />
      <Alert v-if="recovery.error.value" tone="warn">{{ recovery.error.value }}</Alert>
      <div class="actions">
        <span class="muted">Your draft is kept in this tab for this account.</span
        ><Button :disabled="saving" @click="showCreate(false)">Keep draft and close</Button
        ><Button type="submit" variant="primary" :loading="saving" :disabled="!draft.title.trim()"
          >Create issue</Button
        >
      </div>
    </form>
    <form class="filters" @submit.prevent="filters">
      <TextField
        v-model="query"
        type="search"
        label="Search issues"
        placeholder="Title or description"
      /><SelectField v-model="state" label="Status" :options="states" /><Button
        type="submit"
        :disabled="busy"
        >Apply</Button
      >
    </form>
    <p v-if="busy && !rows.length" role="status">Loading issues…</p>
    <EmptyState
      v-else-if="!rows.length && supported"
      icon="search"
      title="No matching issues"
      description="Change the filters, or create an issue to start planning work."
    />
    <ol class="issue-list">
      <li v-for="issue in rows" :key="issue.number">
        <div>
          <RouterLink
            class="issue-title"
            :to="{ name: 'issue', params: { ...routeParams, number: issue.number } }"
            >{{ issue.title }}</RouterLink
          >
          <p class="muted">
            #{{ issue.number }} · {{ issue.author }} · <RelativeTime :ms="issue.created_at_ms" />
            <span v-if="issue.task_id">· linked to an agent task</span>
          </p>
        </div>
        <span class="state" :class="issue.state">{{ issue.state }}</span>
      </li>
    </ol>
    <Button v-if="next" :loading="busy" @click="load(true)">Load more issues</Button>
  </div>
</template>
<style scoped>
.issues-page {
  display: grid;
  gap: var(--s-4);
}
.heading,
.actions,
.filters {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.heading {
  justify-content: space-between;
}
.filters {
  align-items: end;
}
.filters > :first-child {
  flex: 1;
  min-width: 220px;
}
.issue-form {
  display: grid;
  gap: var(--s-3);
  padding: var(--s-4);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.actions {
  justify-content: flex-end;
}
.actions > .muted {
  margin-right: auto;
}
.issue-list {
  list-style: none;
  margin: 0;
  padding: 0;
  border-top: 1px solid var(--border);
}
.issue-list li {
  display: flex;
  justify-content: space-between;
  gap: var(--s-3);
  padding: var(--s-4) 0;
  border-bottom: 1px solid var(--border);
}
.issue-title {
  font-weight: 650;
  font-size: var(--fs-md);
  overflow-wrap: anywhere;
}
.issue-list p {
  margin-top: var(--s-1);
  font-size: var(--fs-sm);
}
.state {
  text-transform: capitalize;
  font-size: var(--fs-sm);
}
.open {
  color: var(--accent);
}
</style>
