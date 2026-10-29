<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import ExecutionControl from '@/components/agents/ExecutionControl.vue'
import TaskEditor from '@/components/agents/TaskEditor.vue'
import TaskAttemptLimit from '@/components/agents/TaskAttemptLimit.vue'
import { api } from '@/api/instance'
import { describeError, isApiError } from '@/api/errors'
import { useRepoName } from '@/composables/useRepoName'
import { usePageTitle } from '@/composables/usePageTitle'
import { useResource } from '@/composables/useResource'
import { usePagedResource } from '@/composables/usePagedResource'
import { useAssignableAgents } from '@/composables/useAssignableAgents'
import { useLiveReload } from '@/composables/useLiveReload'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { samePrincipal } from '@/lib/identity'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import CopyButton from '@/ui/CopyButton.vue'
import Dialog from '@/ui/Dialog.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import SelectField from '@/ui/SelectField.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{ id: string }>()
const { repo, routeParams } = useRepoName()
const auth = useAuth()
const board = useBoard()
const scope = [repo, () => props.id, () => auth.identity]
const task = useResource((signal) => api.agents.task(repo.value, props.id, { signal }), {
  watch: scope,
})
const tasks = useResource((signal) => api.agents.tasks(repo.value, {}, { signal }), {
  watch: scope,
})
const historySupported = computed(() => !!auth.capabilities?.features.includes('tasks.history'))
const historyScope = [...scope, () => auth.token, historySupported]
const runs = usePagedResource(
  (cursor, signal) =>
    api.agents.runPage(
      repo.value,
      {
        limit: historySupported.value ? 50 : 100,
        task_id: historySupported.value ? props.id : undefined,
        page_token: cursor,
      },
      { signal },
    ),
  historyScope,
)
const changes = usePagedResource(
  (cursor, signal) =>
    api.proposals.list(
      repo.value,
      {
        page_size: historySupported.value ? 50 : 100,
        task_id: historySupported.value ? props.id : undefined,
        page_token: cursor,
      },
      { signal },
    ),
  historyScope,
)
usePageTitle(() => `${task.data.value?.title || 'Task'} · ${repo.value}`)
useLiveReload(['task', 'run', 'proposal'], () =>
  Promise.all([task.reload(), tasks.reload(), runs.reload(), changes.reload()]).then(() => {}),
)
const byId = computed(() => new Map((tasks.data.value || []).map((item) => [item.id, item])))
const followUps = computed(() =>
  (tasks.data.value || [])
    .filter((item) => item.source_task === props.id)
    .sort((a, b) => b.created_at_ms - a.created_at_ms),
)
const dependants = computed(() =>
  (tasks.data.value || []).filter((item) => item.depends_on.includes(props.id)),
)
const sessions = computed(() => (runs.data.value || []).filter((run) => run.task_id === props.id))
const proposals = computed(() => changes.data.value.filter((change) => change.task_id === props.id))
const taskLink = (id: string) => ({ name: 'task', params: { ...routeParams.value, id } })
const active = computed(
  () => !!task.data.value && ['open', 'in_progress'].includes(task.data.value.state),
)
const canWrite = computed(() => auth.can(repo.value, 'createTask'))
const canFinish = computed(
  () =>
    canWrite.value &&
    !!task.data.value &&
    (!task.data.value.owner ||
      samePrincipal(task.data.value.owner, auth.identity) ||
      auth.hasRole(repo.value, 'delegator')),
)
const options = computed(() => [
  { value: '', label: 'Choose a prerequisite task' },
  ...(tasks.data.value || [])
    .filter(
      (item) =>
        item.id !== props.id &&
        !task.data.value?.depends_on.includes(item.id) &&
        item.state !== 'cancelled',
    )
    .map((item) => ({ value: item.id, label: `${item.title} · ${item.state}` })),
])
const dependency = ref('')
const busy = ref(false)
const error = ref('')
const message = ref('')
const needsSignIn = ref(false)
const finishing = ref<'done' | 'cancelled' | null>(null)
const assigning = ref(false),
  assignee = ref('')
const assignment = useAssignableAgents(
  () => repo.value,
  () => assigning.value,
)
let expectedOwner = ''
const canAssign = computed(
  () =>
    assignment.supported.value &&
    canWrite.value &&
    task.data.value?.state === 'open' &&
    (samePrincipal(task.data.value.created_by, auth.identity) ||
      auth.hasRole(repo.value, 'delegator') ||
      auth.hasRole(repo.value, 'admin')),
)
function chooseAgent() {
  if (!canAssign.value || !task.data.value) return
  expectedOwner = task.data.value.owner || ''
  assignee.value = expectedOwner.startsWith('agent:') ? expectedOwner.slice(6) : ''
  error.value = ''
  assigning.value = true
}
async function saveAssignment() {
  if (busy.value || !canAssign.value) return
  const mine = context
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    const result = await api.agents.assignTask(
      repo.value,
      props.id,
      assignee.value,
      expectedOwner,
      { token: auth.token, signal: controller.signal, quietUnauthenticated: true },
    )
    if (mine !== context) return
    task.data.value = result
    assigning.value = false
    message.value = result.owner
      ? 'Agent assigned. It can pick up this task when ready and execution is allowed.'
      : 'Task returned to automatic routing.'
    board.touched(['tasks'])
    await tasks.reload()
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  } finally {
    if (mine === context) busy.value = false
  }
}

const windowOrigin = typeof window === 'undefined' ? '' : window.location.origin
const link = computed(() =>
  typeof window === 'undefined'
    ? ''
    : `${window.location.origin}/repos/${repo.value.split('/').map(encodeURIComponent).join('/')}/tasks/${encodeURIComponent(props.id)}`,
)
let context = 0
let controller: AbortController | undefined
watch(
  [repo, () => props.id, () => auth.identity, () => auth.token],
  () => {
    context++
    controller?.abort()
    busy.value = false
    finishing.value = null
    assigning.value = false
    assignee.value = ''
    dependency.value = ''
    error.value = ''
    message.value = ''
    needsSignIn.value = false
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
})
async function change(kind: 'dependency' | 'done' | 'cancelled') {
  if (
    busy.value ||
    needsSignIn.value ||
    !active.value ||
    !canWrite.value ||
    (kind !== 'dependency' && !canFinish.value) ||
    (kind === 'dependency' && !dependency.value)
  )
    return
  const mine = context
  const repository = repo.value
  const id = props.id
  const selected = dependency.value
  controller = new AbortController()
  const request = { token: auth.token, signal: controller.signal, quietUnauthenticated: true }
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    const result =
      kind === 'dependency'
        ? await api.agents.addDependency(repository, id, selected, request)
        : await api.agents.finishTask(repository, id, kind, request)
    if (mine !== context) return
    task.data.value = result
    dependency.value = ''
    finishing.value = null
    message.value =
      kind === 'dependency'
        ? 'Prerequisite added.'
        : kind === 'done'
          ? 'Task marked done.'
          : 'Task cancelled.'
    board.touched(['tasks'])
    await tasks.reload()
  } catch (cause) {
    if (mine === context) {
      error.value = describeError(cause)
      needsSignIn.value = isApiError(cause) && cause.isUnauthenticated
    }
  } finally {
    if (mine === context) busy.value = false
  }
}
</script>

<template>
  <div class="task-page">
    <RouterLink :to="{ name: 'agents', params: routeParams, query: { tab: 'tasks' } }"
      >Back to tasks</RouterLink
    >
    <ErrorState
      v-if="task.error.value && !task.data.value"
      what="this task"
      :error="task.error.value"
      @retry="task.reload()"
    />
    <Skeleton v-else-if="!task.data.value" :lines="6" />
    <template v-else>
      <header class="heading">
        <div>
          <h1>{{ task.data.value.title }}</h1>
          <p class="muted">
            Created by <IdentityLabel :identity="task.data.value.created_by" /> ·
            <RelativeTime :ms="task.data.value.created_at_ms" />
          </p>
        </div>
        <div class="actions">
          <Button
            v-if="canWrite"
            :to="{ name: 'agents', params: routeParams, query: { tab: 'tasks', from_task: id } }"
            >Create a task from this one…</Button
          >
          <TaskEditor
            :repo="repo"
            :task="task.data.value"
            :has-runs="sessions.length > 0"
            @saved="
              (updated) => {
                task.data.value = updated
                board.touched(['tasks'])
                task.reload()
                tasks.reload()
              }
            "
            @reload="task.reload()"
          />
          <CopyButton :text="link" label="Copy task link" with-label />
        </div>
      </header>
      <div class="state">
        <Badge :tone="task.data.value.state === 'done' ? 'ok' : 'neutral'">{{
          task.data.value.state
        }}</Badge
        ><span v-if="active">{{
          task.data.value.max_attempts > 0 &&
          task.data.value.attempts_started >= task.data.value.max_attempts
            ? 'No further attempts available under the current limit.'
            : task.data.value.ready
              ? 'All prerequisites are complete.'
              : 'Waiting for unfinished prerequisites.'
        }}</span
        ><span v-if="task.data.value.owner"
          >{{ task.data.value.state === 'open' ? 'Assigned to' : 'Owned by' }}
          <IdentityLabel :identity="task.data.value.owner" /></span
        ><span v-else>Automatic routing</span
        ><Button v-if="canAssign" size="sm" :disabled="busy" @click="chooseAgent"
          >Choose agent…</Button
        >
      </div>
      <section v-if="task.data.value.source_task" aria-labelledby="task-source-heading">
        <h2 id="task-source-heading">Source of this task</h2>
        <p>
          Created from
          <RouterLink :to="taskLink(task.data.value.source_task)">{{
            byId.get(task.data.value.source_task)?.title || task.data.value.source_task
          }}</RouterLink>
          <template v-if="task.data.value.source_run">
            ·
            <RouterLink
              :to="{ name: 'run', params: { ...routeParams, id: task.data.value.source_run } }"
              >View source run</RouterLink
            >
          </template>
        </p>
        <p class="muted">
          This task has its own agent assignment, prerequisites and attempt history.
        </p>
      </section>
      <p v-if="task.data.value.description" class="description">
        {{ task.data.value.description }}
      </p>
      <p v-if="task.data.value.scope.length" class="state">
        Scope <code v-for="path in task.data.value.scope" :key="path">{{ path }}</code>
      </p>
      <TaskAttemptLimit
        :repo="repo"
        :task="task.data.value"
        @saved="
          (updated) => {
            task.data.value = updated
            board.touched(['tasks'])
            tasks.reload()
          }
        "
        @reload="task.reload()"
      />
      <ExecutionControl :repo="repo" />
      <Alert
        v-if="error && !finishing && !assigning"
        tone="bad"
        live="alert"
        title="The task was not updated"
        >{{ error
        }}<a
          v-if="needsSignIn"
          :href="'/signin?next=' + encodeURIComponent(link.replace(windowOrigin, ''))"
          >Sign in again</a
        ></Alert
      >
      <p v-if="message" role="status">{{ message }}</p>
      <div class="columns">
        <section aria-label="Task prerequisites">
          <h2>Prerequisites</h2>
          <ErrorState
            v-if="tasks.error.value"
            compact
            what="task dependencies"
            :error="tasks.error.value"
            @retry="tasks.reload()"
          />
          <ul v-if="task.data.value.depends_on.length" class="items">
            <li v-for="dependencyId in task.data.value.depends_on" :key="dependencyId">
              <RouterLink :to="taskLink(dependencyId)">{{
                byId.get(dependencyId)?.title || dependencyId
              }}</RouterLink
              ><Badge size="sm" :tone="byId.get(dependencyId)?.state === 'done' ? 'ok' : 'warn'">{{
                byId.get(dependencyId)?.state || 'Not loaded'
              }}</Badge>
            </li>
          </ul>
          <p v-else class="muted">No prerequisites.</p>
          <form
            v-if="active && canWrite"
            class="dependency-form"
            @submit.prevent="change('dependency')"
          >
            <SelectField
              v-model="dependency"
              label="Add prerequisite"
              :options="options"
              :disabled="busy || tasks.loading.value"
            /><Button type="submit" :disabled="busy || !dependency || needsSignIn"
              >Add prerequisite</Button
            >
          </form>
          <p v-if="active && canWrite" class="muted hint">
            The task will wait for this prerequisite. Circular dependencies are rejected by the
            server.
          </p>
        </section>
        <section aria-label="Dependent tasks">
          <h2>Waiting on this task</h2>
          <ul v-if="dependants.length" class="items">
            <li v-for="item in dependants" :key="item.id">
              <RouterLink :to="taskLink(item.id)">{{ item.title }}</RouterLink
              ><Badge size="sm">{{ item.state }}</Badge>
            </li>
          </ul>
          <p v-else class="muted">No loaded tasks depend on this task.</p>
        </section>
      </div>
      <section v-if="followUps.length" aria-label="Follow-up tasks">
        <h2>Follow-up tasks</h2>
        <ul class="items">
          <li v-for="item in followUps" :key="item.id">
            <RouterLink :to="taskLink(item.id)">{{ item.title }}</RouterLink>
            <Badge size="sm">{{ item.state }}</Badge>
          </li>
        </ul>
      </section>
      <section aria-label="Related change requests">
        <h2>Change requests</h2>
        <p class="muted hint">
          {{
            historySupported
              ? 'Changes linked to this task, newest first.'
              : 'This server does not support task filtering. Matches are from the loaded repository history.'
          }}
        </p>
        <ErrorState
          v-if="changes.error.value"
          compact
          what="related change requests"
          :error="changes.error.value"
          @retry="changes.reload()"
        />
        <ul v-if="proposals.length" class="items">
          <li v-for="proposal in proposals" :key="proposal.id">
            <RouterLink :to="{ name: 'proposal', params: { ...routeParams, id: proposal.id } }">{{
              proposal.title
            }}</RouterLink
            ><Badge size="sm">{{ proposal.state }}</Badge>
          </li>
        </ul>
        <p v-else-if="!changes.loading.value" class="muted">
          No matching change requests in the loaded history.
        </p>
        <p v-if="changes.loading.value && !changes.data.value.length" role="status">
          Loading change requests…
        </p>
        <Button v-if="changes.next.value" :loading="changes.loading.value" @click="changes.more"
          >Load older change requests</Button
        >
      </section>
      <section aria-label="Related agent sessions">
        <h2>Agent sessions</h2>
        <p class="muted hint">
          {{
            historySupported
              ? 'Sessions linked to this task, newest first.'
              : 'This server does not support task filtering. Matches are from the loaded repository history.'
          }}
        </p>
        <ErrorState
          v-if="runs.error.value"
          compact
          what="agent sessions"
          :error="runs.error.value"
          @retry="runs.reload()"
        />
        <ul v-if="sessions.length" class="items">
          <li v-for="run in sessions" :key="run.run_id">
            <div>
              <IdentityLabel :identity="run.identity" />
              <p v-if="run.summary">{{ run.summary }}</p>
              <RouterLink
                v-if="auth.capabilities?.features.includes('agents.run_lookup')"
                :to="{ name: 'run', params: { ...routeParams, id: run.run_id } }"
                >{{ run.run_id }}</RouterLink
              >
              <code v-if="!auth.capabilities?.features.includes('agents.run_lookup')">{{
                run.run_id
              }}</code>
            </div>
            <Badge size="sm">{{ run.status }}</Badge>
          </li>
        </ul>
        <p v-else-if="!runs.loading.value" class="muted">
          No matching sessions in the loaded history.
        </p>
        <p v-if="runs.loading.value && !runs.data.value.length" role="status">
          Loading agent sessions…
        </p>
        <Button v-if="runs.next.value" :loading="runs.loading.value" @click="runs.more"
          >Load older sessions</Button
        >
      </section>
      <footer v-if="active" class="actions">
        <Button
          :disabled="busy || !canFinish || needsSignIn"
          :disabled-reason="
            !canFinish ? 'Only the task owner or an operator can finish an owned task.' : undefined
          "
          @click="finishing = 'done'"
          >Mark done…</Button
        ><Button
          variant="danger"
          :disabled="busy || !canFinish || needsSignIn"
          @click="finishing = 'cancelled'"
          >Cancel task…</Button
        >
      </footer>
    </template>
    <Dialog
      :model-value="finishing !== null"
      :title="finishing === 'done' ? 'Mark this task done?' : 'Cancel this task?'"
      :description="
        finishing === 'done'
          ? 'Dependent tasks may become ready to start. This records task completion; it does not approve or land any code.'
          : 'Dependent tasks will keep waiting. Connected orchestrators stop their managed sessions after observing cancellation; manually launched processes may need a separate stop.'
      "
      :dismissable="!busy"
      @update:model-value="finishing = null"
      ><p>{{ task.data.value?.title }}</p>
      <Alert v-if="error" tone="bad" live="alert" title="The task was not updated"
        >{{ error
        }}<a
          v-if="needsSignIn"
          :href="'/signin?next=' + encodeURIComponent(link.replace(windowOrigin, ''))"
          >Sign in again</a
        ></Alert
      >
      <template #footer
        ><Button :disabled="busy" @click="finishing = null">Keep task unchanged</Button
        ><Button
          :variant="finishing === 'cancelled' ? 'danger' : 'primary'"
          :loading="busy"
          :disabled="needsSignIn"
          @click="finishing && change(finishing)"
          >Confirm {{ finishing === 'done' ? 'completion' : 'cancellation' }}</Button
        ></template
      ></Dialog
    >
    <Dialog v-model="assigning" title="Choose an agent" :busy="busy">
      <p>
        The assignment is reserved before execution. A configured executor for the chosen agent can
        start when the task is ready and task execution is allowed.
      </p>
      <SelectField
        v-model="assignee"
        label="Assigned agent"
        :options="assignment.options.value"
        :disabled="busy || assignment.agents.loading.value"
      />
      <ErrorState
        v-if="assignment.agents.error.value"
        :error="assignment.agents.error.value"
        @retry="assignment.agents.reload()"
      />
      <Alert v-if="error" tone="bad"
        >{{ error }} Close this dialog and review the current assignment before retrying.</Alert
      >
      <template #footer
        ><Button :disabled="busy" @click="assigning = false">Keep current assignment</Button
        ><Button
          variant="primary"
          :loading="busy"
          :disabled="
            !canAssign ||
            assignment.agents.loading.value ||
            (assignee ? 'agent:' + assignee : '') === expectedOwner
          "
          @click="saveAssignment"
          >Save assignment</Button
        ></template
      >
    </Dialog>
  </div>
</template>

<style scoped>
.task-page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.heading,
.state,
.actions {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.heading {
  justify-content: space-between;
}
.heading h1 {
  overflow-wrap: anywhere;
}
.description {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.columns {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: var(--s-5);
}
section {
  border-top: 1px solid var(--border);
  padding-top: var(--s-3);
  min-width: 0;
}
h2 {
  font-size: var(--fs-md);
  margin-bottom: var(--s-3);
}
.items {
  list-style: none;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.items li {
  display: flex;
  justify-content: space-between;
  gap: var(--s-3);
  align-items: start;
  overflow-wrap: anywhere;
}
.items code {
  overflow-wrap: anywhere;
}
.dependency-form {
  display: flex;
  align-items: end;
  gap: var(--s-2);
  margin-top: var(--s-3);
}
.dependency-form > :first-child {
  flex: 1;
  min-width: 0;
}
.hint {
  font-size: var(--fs-xs);
  margin-top: var(--s-2);
}
.actions {
  border-top: 1px solid var(--border);
  padding-top: var(--s-3);
}
@media (max-width: 720px) {
  .columns {
    grid-template-columns: minmax(0, 1fr);
  }
}
</style>
