<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { randomHex } from '@/api/client'
import { useAssignableAgents } from '@/composables/useAssignableAgents'
import { ApiError, describeError } from '@/api/errors'
import type { Task, Run } from '@/api/types'
import { useLiveReload } from '@/composables/useLiveReload'
import { useResource } from '@/composables/useResource'
import { followUpDescription } from '@/lib/taskRecovery'
import { requirementText } from '@/lib/roles'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Checkbox from '@/ui/Checkbox.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'
import SelectField from '@/ui/SelectField.vue'

const props = defineProps<{ repo: string }>()
const auth = useAuth()
const route = useRoute(),
  router = useRouter()
const copiedFrom = ref<Task | null>(null)
const copiedRun = ref<Run | null>(null)
let handledSource = ''
const board = useBoard()
const toasts = useToasts()

const tasks = useResource<Task[]>(() => api.agents.tasks(props.repo), {
  watch: [() => props.repo, () => auth.token],
})
useLiveReload(['task', 'run'], () => tasks.reload())

const COLUMNS = [
  { id: 'open', title: 'Open', hint: 'Queued or waiting for prerequisites' },
  { id: 'in_progress', title: 'In progress', hint: 'Claimed by a run' },
  { id: 'done', title: 'Done', hint: 'Finished' },
  { id: 'cancelled', title: 'Cancelled', hint: 'Dropped' },
] as const
const byId = computed(() => new Map((tasks.data.value ?? []).map((t) => [t.id, t])))
const grouped = computed(() => {
  const g: Record<string, Task[]> = { open: [], in_progress: [], done: [], cancelled: [] }
  for (const t of tasks.data.value ?? []) (g[t.state] ??= []).push(t)
  g.open?.sort((a, b) => Number(b.ready) - Number(a.ready) || a.created_at_ms - b.created_at_ms)
  g.done?.sort((a, b) => b.updated_at_ms - a.updated_at_ms)
  return g
})
const waitingFor = (t: Task) => t.depends_on.filter((d) => byId.value.get(d)?.state !== 'done')

// ---- create -----------------------------------------------------------------------------------------
const creating = ref(false)
const assignee = ref('')
const assignment = useAssignableAgents(
  () => props.repo,
  () => creating.value,
)
const lineageSupported = computed(() => !!auth.capabilities?.features.includes('tasks.lineage'))
const title = ref('')
const description = ref('')
const scope = ref('')
const depends = ref<string[]>([])
const error = ref<ApiError | null>(null)
const busy = ref(false)
let context = 0,
  controller: AbortController | undefined,
  requestId = randomHex()
watch(
  [title, description, scope, depends, assignee, copiedFrom, copiedRun, lineageSupported],
  () => {
    requestId = randomHex()
  },
  { deep: true, flush: 'sync' },
)
watch(
  [() => props.repo, () => auth.token],
  () => {
    context++
    controller?.abort()
    creating.value = false
    busy.value = false
    title.value = ''
    description.value = ''
    scope.value = ''
    depends.value = []
    assignee.value = ''
    error.value = null
    copiedFrom.value = null
    copiedRun.value = null
    handledSource = ''
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
})
const canCreate = computed(() => auth.can(props.repo, 'createTask'))
const sourceTaskId = computed(() =>
  typeof route.query.from_task === 'string' ? route.query.from_task : '',
)
const sourceRunId = computed(() =>
  typeof route.query.from_run === 'string' ? route.query.from_run : '',
)
const sourceTask = useResource(
  (signal) => api.agents.task(props.repo, sourceTaskId.value, { signal }),
  {
    watch: [() => props.repo, () => auth.token, sourceTaskId, canCreate],
    enabled: () => !!sourceTaskId.value && canCreate.value,
  },
)
const sourceRun = useResource(
  (signal) => api.agents.run(props.repo, sourceRunId.value, { signal }),
  {
    watch: [() => props.repo, () => auth.token, sourceRunId, canCreate],
    enabled: () => !!sourceRunId.value && !!sourceTaskId.value && canCreate.value,
  },
)
const sourceMismatch = computed(
  () =>
    sourceRun.data.value &&
    (sourceRun.data.value.task_id !== sourceTaskId.value ||
      !['failed', 'abandoned'].includes(sourceRun.data.value.status)),
)
const candidates = computed(() =>
  (tasks.data.value ?? []).filter((t) => t.state !== 'cancelled' && t.state !== 'done'),
)

function openCreate(source?: Task, run?: Run) {
  copiedRun.value = run ?? null
  copiedFrom.value = source ?? null
  assignee.value = ''
  title.value = source?.title ?? ''
  description.value =
    source && run
      ? followUpDescription(props.repo, source, run, !lineageSupported.value)
      : (source?.description ?? '')
  scope.value = source?.scope.join('\n') ?? ''
  depends.value = []
  error.value = null
  creating.value = true
}
watch(
  [sourceTaskId, sourceRunId, () => sourceTask.data.value, () => sourceRun.data.value, canCreate],
  () => {
    const source = sourceTask.data.value
    if (!source || source.id !== sourceTaskId.value || !canCreate.value || creating.value) return
    const run = sourceRun.data.value
    if (sourceRunId.value && (!run || run.run_id !== sourceRunId.value || sourceMismatch.value))
      return
    const key = `${props.repo}:${source.id}:${sourceRunId.value}`
    if (handledSource === key) return
    handledSource = key
    openCreate(source, sourceRunId.value ? run : undefined)
  },
  { immediate: true },
)
function clearSourceQuery() {
  if (route.query.from_task)
    void router.replace({ query: { ...route.query, from_task: undefined, from_run: undefined } })
}
function dismissCreate() {
  if (busy.value) return
  creating.value = false
  clearSourceQuery()
}

async function create() {
  if (busy.value || !canCreate.value || !title.value.trim()) return
  const mine = context
  controller = new AbortController()
  const request = {
    token: auth.token,
    signal: controller.signal,
    idempotencyKey: requestId,
    quietUnauthenticated: true,
  }
  busy.value = true
  error.value = null
  try {
    const created = await api.agents.createTask(
      props.repo,
      {
        title: title.value.trim(),
        description: description.value.trim() || undefined,
        scope: scope.value
          .split(/[\n,]+/)
          .map((s) => s.trim())
          .filter(Boolean),
        depends_on: depends.value,
        assignee: assignment.supported.value ? assignee.value : undefined,
        source_task: lineageSupported.value ? copiedFrom.value?.id : undefined,
        source_run: lineageSupported.value ? copiedRun.value?.run_id : undefined,
      },
      request,
    )
    if (mine !== context) return
    const reused = copiedFrom.value !== null
    creating.value = false
    toasts.ok('Task created')
    board.touched(['tasks'])
    if (reused) {
      await router.push({ name: 'task', params: { ...route.params, id: created.id }, query: {} })
    } else {
      clearSourceQuery()
      await tasks.reload()
    }
  } catch (e) {
    if (mine !== context) return
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    if (mine === context) busy.value = false
  }
}

const toggle = (id: string, on: boolean) => {
  depends.value = on ? [...depends.value, id] : depends.value.filter((x) => x !== id)
}
</script>

<template>
  <div class="tasks">
    <div class="bar">
      <p class="muted">
        Tasks give an agent ownership of a piece of work. A task is <em>ready</em> when everything
        it depends on is done.
      </p>
      <Button
        icon="plus"
        variant="primary"
        :disabled="!canCreate"
        :disabled-reason="requirementText('createTask')"
        @click="openCreate()"
        >New task</Button
      >
    </div>
    <ErrorState
      v-if="sourceTask.error.value"
      what="source task"
      :error="sourceTask.error.value"
      @retry="sourceTask.reload()"
    />
    <ErrorState
      v-if="sourceRun.error.value"
      what="source run"
      :error="sourceRun.error.value"
      @retry="sourceRun.reload()"
    />
    <Alert v-if="sourceMismatch" tone="bad" title="Cannot use this run"
      >The run must be a failed or abandoned session of the source task. Open the original run page
      to choose the correct source.</Alert
    >
    <ErrorState
      v-if="tasks.error.value && !tasks.data.value"
      what="tasks"
      :error="tasks.error.value"
      @retry="tasks.reload()"
    />
    <div v-else-if="!tasks.settled.value" class="cols">
      <Skeleton v-for="n in 4" :key="n" :height="180" radius="10px" />
    </div>
    <div v-else class="cols" data-testid="task-board">
      <section v-for="c in COLUMNS" :key="c.id" class="col" :aria-labelledby="`col-${c.id}`">
        <header>
          <h3 :id="`col-${c.id}`">{{ c.title }}</h3>
          <Badge size="sm">{{ grouped[c.id]?.length ?? 0 }}</Badge>
        </header>
        <p class="faint hint">{{ c.hint }}</p>
        <ul role="list" class="list">
          <li v-for="t in grouped[c.id]" :key="t.id" class="task" :data-task="t.id">
            <p class="t">
              <RouterLink
                :to="`/repos/${repo.split('/').map(encodeURIComponent).join('/')}/tasks/${encodeURIComponent(t.id)}`"
                >{{ t.title }}</RouterLink
              >
            </p>
            <p v-if="t.description" class="d">{{ t.description }}</p>
            <p class="chips">
              <Badge
                v-if="c.id === 'open'"
                size="sm"
                :tone="t.ready ? 'ok' : 'warn'"
                :icon="t.ready ? 'check' : 'lock'"
                >{{
                  t.max_attempts > 0 && t.attempts_started >= t.max_attempts
                    ? 'attempt limit reached'
                    : t.ready
                      ? 'ready'
                      : `waiting for ${waitingFor(t).length}`
                }}</Badge
              >
              <Badge v-for="s in t.scope" :key="s" size="sm" mono>{{ s }}</Badge>
            </p>
            <p v-if="waitingFor(t).length" class="deps faint">
              after:
              {{
                waitingFor(t)
                  .map((d) => byId.get(d)?.title ?? d)
                  .join(', ')
              }}
            </p>
            <p class="foot faint">
              <IdentityLabel v-if="t.owner" :identity="t.owner" :size="16" />
              <span v-else>unowned</span>
              <RelativeTime :ms="t.updated_at_ms" />
            </p>
          </li>
          <li v-if="!grouped[c.id]?.length" class="none">
            <EmptyState compact icon="circle" title="None" />
          </li>
        </ul>
      </section>
    </div>

    <Dialog
      :model-value="creating"
      :title="copiedFrom ? 'Create a task from this one' : 'New task'"
      :dismissable="!busy"
      size="lg"
      @update:model-value="dismissCreate"
    >
      <p v-if="copiedFrom">
        Title, description and file scope copied from
        <RouterLink
          :to="{ name: 'task', params: { ...route.params, id: copiedFrom.id }, query: {} }"
          >{{ copiedFrom.title }}</RouterLink
        >. This creates a separate task with its own run history and retry budget. Choose its agent
        and prerequisites below; the source task stays unchanged.
      </p>
      <p v-if="copiedRun">
        Failure context from run
        <RouterLink
          :to="{ name: 'run', params: { ...route.params, id: copiedRun.run_id }, query: {} }"
          >{{ copiedRun.run_id }}</RouterLink
        >
        is included in the editable description.
        {{
          lineageSupported
            ? 'Its source links are saved separately.'
            : 'The description also keeps its source links.'
        }}
        Review the cause and adjust the expected result before creating the follow-up.
      </p>
      <form id="new-task" class="form" @submit.prevent="create">
        <SelectField
          v-if="assignment.supported.value"
          v-model="assignee"
          label="Assigned agent"
          :options="assignment.options.value"
          :disabled="busy || assignment.agents.loading.value"
          hint="Configured executors may pick up ready tasks after you create them."
        />
        <ErrorState
          v-if="assignment.agents.error.value"
          compact
          what="available agents"
          :error="assignment.agents.error.value"
          @retry="assignment.agents.reload()"
        />
        <TextField
          v-model="title"
          :disabled="busy"
          label="Title"
          required
          autofocus
          placeholder="Extract the session store"
        />
        <TextArea v-model="description" :disabled="busy" label="Description" :rows="3" />
        <TextArea
          v-model="scope"
          :disabled="busy"
          label="Scope (path globs)"
          :rows="2"
          mono
          placeholder="src/auth/**"
          hint="One per line or comma-separated. Helps predict who overlaps whom."
        />
        <fieldset v-if="candidates.length" class="deps-set">
          <legend>Depends on</legend>
          <Checkbox
            v-for="t in candidates"
            :key="t.id"
            :disabled="busy"
            :model-value="depends.includes(t.id)"
            :label="t.title"
            @update:model-value="toggle(t.id, $event)"
          />
        </fieldset>
        <Alert v-if="error" tone="bad" live="alert" title="Could not create the task">{{
          error.message
        }}</Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="dismissCreate">Cancel</Button>
        <Button type="submit" form="new-task" variant="primary" :loading="busy">Create task</Button>
      </template>
    </Dialog>
  </div>
</template>

<style scoped>
.tasks {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.cols {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
  gap: var(--s-3);
  align-items: start;
}
.col {
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-3);
  min-width: 0;
}
.col header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
h3 {
  font-size: var(--fs-md);
}
.hint {
  font-size: var(--fs-xs);
  margin-bottom: var(--s-2);
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.task {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.t {
  font-weight: 600;
  overflow-wrap: anywhere;
}
.d {
  font-size: var(--fs-sm);
  color: var(--text-2);
  overflow-wrap: anywhere;
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.deps {
  font-size: var(--fs-xs);
}
.foot {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-xs);
}
.none {
  list-style: none;
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.deps-set {
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  max-height: 200px;
  overflow: auto;
}
legend {
  font-weight: 580;
  font-size: var(--fs-sm);
  padding: 0 4px;
}
</style>
