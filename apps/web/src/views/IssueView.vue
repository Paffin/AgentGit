<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api } from '@/api/instance'
import { randomHex } from '@/api/client'
import type { IssueActivity } from '@/api/types'
import { describeError, isApiError } from '@/api/errors'
import { useRepoName } from '@/composables/useRepoName'
import { useResource } from '@/composables/useResource'
import { usePageTitle } from '@/composables/usePageTitle'
import { useLiveReload } from '@/composables/useLiveReload'
import { useAssignableAgents } from '@/composables/useAssignableAgents'
import { useIssueDraft } from '@/composables/useIssueDraft'
import { useAuth } from '@/stores/auth'
import { samePrincipal } from '@/lib/identity'
import Button from '@/ui/Button.vue'
import Alert from '@/ui/Alert.vue'
import ErrorState from '@/ui/ErrorState.vue'
import TextField from '@/ui/TextField.vue'
import TextArea from '@/ui/TextArea.vue'
import SelectField from '@/ui/SelectField.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Dialog from '@/ui/Dialog.vue'
const props = defineProps<{ number: string }>()
const auth = useAuth(),
  route = useRoute()
const { repo, routeParams } = useRepoName()
const scope = computed(() => JSON.stringify([auth.identity, repo.value, props.number]))
const watched = [repo, () => props.number, () => auth.token]
const issue = useResource(
  (signal) =>
    api.issues.get(repo.value, Number(props.number), {
      signal,
      token: auth.token,
      quietUnauthenticated: true,
    }),
  { watch: watched },
)
const tasks = useResource((signal) => api.agents.tasks(repo.value, {}, { signal }), {
  watch: watched,
})
const editDraft = useIssueDraft(() => (auth.identity ? scope.value + ':edit' : ''))
const commentDraft = useIssueDraft(() => (auth.identity ? scope.value + ':comment' : ''))
const editing = ref(false),
  busy = ref(false),
  error = ref(''),
  message = ref(''),
  confirmState = ref(false)
const taskDialog = ref(false)
const taskAssignee = ref('')
const assignment = useAssignableAgents(
  () => repo.value,
  () => taskDialog.value,
)
const taskSourceVersion = ref(0)
const taskTitle = ref(''),
  taskDescription = ref(''),
  taskScope = ref('')
let taskRequestId = randomHex()
watch(
  [taskTitle, taskDescription, taskScope, taskAssignee],
  () => {
    taskRequestId = randomHex()
  },
  { flush: 'sync' },
)
const activity = ref<IssueActivity[]>([]),
  next = ref(''),
  loadingActivity = ref(false)
let activitySerial = 0,
  activityController: AbortController | undefined,
  mutationController: AbortController | undefined
const canWrite = computed(() => auth.can(repo.value, 'createIssue'))
const canEdit = computed(
  () =>
    canWrite.value &&
    !!issue.data.value &&
    (auth.hasRole(repo.value, 'admin') || samePrincipal(issue.data.value.author, auth.identity)),
)
const taskOptions = computed(() => [
  { value: '', label: 'No linked task' },
  ...(tasks.data.value || []).map((t) => ({ value: t.id, label: `${t.title} · ${t.state}` })),
])
const linkedTask = computed(() =>
  (tasks.data.value || []).find((t) => t.id === issue.data.value?.task_id),
)
usePageTitle(() => `${issue.data.value?.title || 'Issue'} · ${repo.value}`)
async function loadActivity(append = false) {
  activityController?.abort()
  activityController = new AbortController()
  const mine = ++activitySerial
  loadingActivity.value = true
  try {
    const result = await api.issues.activity(
      repo.value,
      Number(props.number),
      { page_size: 50, page_token: append ? next.value : undefined },
      { signal: activityController.signal, token: auth.token, quietUnauthenticated: true },
    )
    if (mine !== activitySerial) return
    activity.value = append ? [...activity.value, ...result.activity] : result.activity
    next.value = result.next_page_token
  } catch (e) {
    if (mine === activitySerial) error.value = describeError(e)
  } finally {
    if (mine === activitySerial) loadingActivity.value = false
  }
}
watch(
  scope,
  () => {
    editing.value = false
    busy.value = false
    confirmState.value = false
    taskDialog.value = false
    error.value = ''
    message.value = ''
    activity.value = []
    next.value = ''
    mutationController?.abort()
    void loadActivity()
  },
  { immediate: true },
)
onBeforeUnmount(() => {
  activitySerial++
  activityController?.abort()
  mutationController?.abort()
})
function edit() {
  const row = issue.data.value
  if (!row) return
  if (!editDraft.saved.value)
    editDraft.draft.value = {
      ...editDraft.draft.value,
      title: row.title,
      body: row.body,
      taskId: row.task_id,
      version: row.version,
    }
  editing.value = true
  message.value = ''
}
async function refresh() {
  await Promise.all([issue.reload(), loadActivity()])
}
async function save(changeState = false) {
  const row = issue.data.value
  if (!row || busy.value || !canEdit.value) return
  const mine = scope.value
  const token = auth.token
  mutationController = new AbortController()
  const draft = editDraft.draft.value
  const body = changeState
    ? {
        title: row.title,
        body: row.body,
        task_id: row.task_id,
        state: row.state === 'open' ? 'closed' : 'open',
        expected_version: row.version,
      }
    : {
        title: draft.title,
        body: draft.body,
        task_id: draft.taskId,
        state: row.state,
        expected_version: draft.version,
      }
  busy.value = true
  error.value = ''
  message.value = ''
  try {
    await api.issues.update(repo.value, row.number, body, {
      token,
      signal: mutationController.signal,
      quietUnauthenticated: true,
    })
    if (mine !== scope.value) return
    if (!changeState) {
      editDraft.clear()
      editing.value = false
    }
    confirmState.value = false
    message.value = changeState ? 'Issue status updated.' : 'Issue saved.'
    await refresh()
  } catch (e) {
    if (mine === scope.value)
      error.value =
        isApiError(e) && e.status === 412
          ? 'Someone changed this issue. Your draft is kept. Reload the issue and review the current version before saving again.'
          : describeError(e)
  } finally {
    if (mine === scope.value) busy.value = false
  }
}
function useLatestVersion() {
  if (issue.data.value) editDraft.draft.value.version = issue.data.value.version
}
function prepareTask() {
  const row = issue.data.value
  if (!row) return
  taskSourceVersion.value = row.version
  taskTitle.value = row.title
  taskDescription.value = row.body
  taskScope.value = ''
  taskAssignee.value = ''
  taskDialog.value = true
  error.value = ''
}
async function createAgentTask() {
  const row = issue.data.value
  const paths = taskScope.value
    .split('\n')
    .map((path) => path.trim())
    .filter(Boolean)
  if (
    !row ||
    busy.value ||
    !taskTitle.value.trim() ||
    !taskDescription.value.trim() ||
    !paths.length
  )
    return
  const mine = scope.value
  mutationController = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.agents.createTask(
      repo.value,
      {
        title: taskTitle.value.trim(),
        description: taskDescription.value,
        scope: paths,
        source_issue: row.number,
        expected_issue_version: taskSourceVersion.value,
        assignee: assignment.supported.value ? taskAssignee.value : undefined,
      },
      {
        token: auth.token,
        signal: mutationController.signal,
        idempotencyKey: taskRequestId,
        quietUnauthenticated: true,
      },
    )
    if (mine !== scope.value) return
    taskDialog.value = false
    await Promise.all([refresh(), tasks.reload()])
    message.value =
      'Agent task created and linked. Open it to follow execution and resulting changes.'
  } catch (e) {
    if (mine === scope.value) error.value = describeError(e)
  } finally {
    if (mine === scope.value) busy.value = false
  }
}
async function comment() {
  if (!canWrite.value || busy.value || !commentDraft.draft.value.body.trim()) return
  const mine = scope.value
  const token = auth.token
  const draft = { ...commentDraft.draft.value }
  mutationController = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.issues.comment(repo.value, Number(props.number), draft.body, {
      token,
      idempotencyKey: draft.requestId,
      signal: mutationController.signal,
      quietUnauthenticated: true,
    })
    if (mine !== scope.value) return
    commentDraft.clear()
    await loadActivity()
    message.value = 'Comment added.'
  } catch (e) {
    if (mine === scope.value) error.value = describeError(e)
  } finally {
    if (mine === scope.value) busy.value = false
  }
}
const actionText: Record<string, string> = {
  created: 'opened this issue',
  updated: 'updated this issue',
  closed: 'closed this issue',
  reopened: 'reopened this issue',
  task_linked: 'linked an agent task',
  commented: 'commented',
}
useLiveReload(['issue'], refresh)
</script>
<template>
  <div class="issue-page">
    <RouterLink :to="{ name: 'issues', params: routeParams }">← All issues</RouterLink>
    <ErrorState v-if="issue.error.value" :error="issue.error.value" @retry="refresh" />
    <p v-else-if="!issue.data.value" role="status">Loading issue…</p>
    <template v-else>
      <header class="heading">
        <div>
          <h1>
            {{ issue.data.value.title }} <span class="muted">#{{ issue.data.value.number }}</span>
          </h1>
          <p class="muted">
            <strong>{{ issue.data.value.state }}</strong> · {{ issue.data.value.author }} opened
            <RelativeTime :ms="issue.data.value.created_at_ms" />
          </p>
        </div>
        <div class="actions">
          <Button :disabled="busy" @click="refresh">Refresh</Button
          ><Button v-if="canEdit && !editing" @click="edit">Edit</Button
          ><Button v-if="canEdit" :disabled="busy" @click="confirmState = true">{{
            issue.data.value.state === 'open' ? 'Close issue' : 'Reopen issue'
          }}</Button>
        </div>
      </header>
      <Alert v-if="error" tone="bad" title="Action could not finish" live="alert"
        >{{ error }} <Button size="sm" @click="refresh">Reload current issue</Button
        ><Button :to="{ path: '/signin', query: { next: route.fullPath } }" size="sm"
          >Sign in again</Button
        ></Alert
      >
      <p v-if="message" role="status">{{ message }}</p>
      <form v-if="editing" class="editor" @submit.prevent="save(false)">
        <h2>Edit issue</h2>
        <TextField
          v-model="editDraft.draft.value.title"
          label="Title"
          required
          :disabled="busy"
        /><TextArea
          v-model="editDraft.draft.value.body"
          label="Description"
          :rows="8"
          :disabled="busy"
        />
        <SelectField
          v-model="editDraft.draft.value.taskId"
          label="Linked agent task"
          :options="taskOptions"
          :disabled="busy"
          hint="Issue status and task execution remain independent."
        />
        <Alert v-if="editDraft.error.value" tone="warn">{{ editDraft.error.value }}</Alert>
        <Alert
          v-if="editDraft.draft.value.version !== issue.data.value.version"
          tone="warn"
          title="A newer version exists"
          >Current title: {{ issue.data.value.title }}
          <p>Linked task: {{ issue.data.value.task_id || 'None' }}</p>
          Current description:
          <p class="body">{{ issue.data.value.body || 'No description.' }}</p>
          <Button size="sm" @click="useLatestVersion"
            >Keep my draft against this version</Button
          ></Alert
        >
        <div class="actions">
          <Button :disabled="busy" @click="editing = false">Keep draft and close</Button
          ><Button
            type="submit"
            variant="primary"
            :loading="busy"
            :disabled="
              !editDraft.draft.value.title.trim() ||
              editDraft.draft.value.version !== issue.data.value.version
            "
            >Save issue</Button
          >
        </div>
      </form>
      <section v-else aria-label="Issue description" class="description">
        <p class="body">{{ issue.data.value.body || 'No description yet.' }}</p>
      </section>
      <p v-if="issue.data.value.task_id" class="linked">
        Linked agent task:
        <RouterLink
          :to="{ name: 'task', params: { ...routeParams, id: issue.data.value.task_id } }"
          >{{ linkedTask?.title || issue.data.value.task_id }}</RouterLink
        ><span v-if="linkedTask"> · {{ linkedTask.state }}</span>
      </p>
      <section
        v-if="!issue.data.value.task_id && canEdit && auth.can(repo, 'createTask')"
        class="agent-handoff"
        aria-labelledby="agent-handoff-heading"
      >
        <h2 id="agent-handoff-heading">Turn this request into agent work</h2>
        <p>
          Define the expected result and file scope, then follow the task's runs, checks and change
          requests.
        </p>
        <Button :disabled="busy || issue.data.value.state !== 'open'" @click="prepareTask"
          >Create agent task…</Button
        >
      </section>
      <Dialog v-model="taskDialog" title="Create an agent task" :busy="busy">
        <p>
          A configured orchestrator can pick up this task automatically. Creating it does not mean
          the work has been executed or approved.
        </p>
        <SelectField
          v-if="assignment.supported.value"
          v-model="taskAssignee"
          label="Assigned agent"
          :options="assignment.options.value"
          :disabled="busy || assignment.agents.loading.value"
          hint="The assignment is saved together with the task. Automatic routing uses the configured rules."
        />
        <ErrorState
          v-if="assignment.agents.error.value"
          compact
          what="available agents"
          :error="assignment.agents.error.value"
          @retry="assignment.agents.reload()"
        />
        <TextField v-model="taskTitle" label="Task title" required :disabled="busy" />
        <TextArea
          v-model="taskDescription"
          label="Expected result"
          required
          :rows="5"
          :disabled="busy"
        />
        <TextArea
          v-model="taskScope"
          label="Allowed file paths"
          :rows="3"
          :disabled="busy"
          hint="One path pattern per line, for example src/**. Use ** only when the task needs the entire repository. The executor's credential and policy still limit access."
        />
        <Alert v-if="error" tone="bad">{{ error }}</Alert>
        <template #footer
          ><Button :disabled="busy" @click="taskDialog = false">Cancel</Button
          ><Button
            variant="primary"
            :loading="busy"
            :disabled="!taskTitle.trim() || !taskDescription.trim() || !taskScope.trim()"
            @click="createAgentTask"
            >Create and link task</Button
          ></template
        >
      </Dialog>
      <section aria-labelledby="discussion-heading" class="discussion">
        <h2 id="discussion-heading">Discussion and activity</h2>
        <ol>
          <li v-for="event in activity" :key="event.id">
            <p class="muted">
              {{ event.author }} {{ actionText[event.kind] || event.kind }} ·
              <RelativeTime :ms="event.created_at_ms" />
            </p>
            <p v-if="event.body" class="body">{{ event.body }}</p>
          </li>
        </ol>
        <Button v-if="next" :loading="loadingActivity" @click="loadActivity(true)"
          >Load more activity</Button
        >
        <p v-if="loadingActivity" role="status">Loading activity…</p>
      </section>
      <form v-if="canWrite" class="editor" @submit.prevent="comment">
        <TextArea
          v-model="commentDraft.draft.value.body"
          label="Add a comment"
          :rows="4"
          :disabled="busy"
          hint="Your unfinished comment is kept in this tab."
        /><Alert v-if="commentDraft.error.value" tone="warn">{{ commentDraft.error.value }}</Alert>
        <div class="actions">
          <Button
            type="submit"
            variant="primary"
            :loading="busy"
            :disabled="!commentDraft.draft.value.body.trim()"
            >Comment</Button
          >
        </div>
      </form>
      <p v-else class="muted">You have read-only access to this discussion.</p>
      <Dialog
        v-model="confirmState"
        :title="issue.data.value.state === 'open' ? 'Close this issue?' : 'Reopen this issue?'"
        :busy="busy"
        ><p>This changes the issue status. It does not stop or finish a linked agent task.</p>
        <template #footer
          ><Button :disabled="busy" @click="confirmState = false">Cancel</Button
          ><Button variant="primary" :loading="busy" @click="save(true)">{{
            issue.data.value.state === 'open' ? 'Close issue' : 'Reopen issue'
          }}</Button></template
        ></Dialog
      >
    </template>
  </div>
</template>
<style scoped>
.agent-handoff {
  border-top: 1px solid var(--border);
  padding-top: var(--s-4);
}
.issue-page {
  display: grid;
  gap: var(--s-4);
  max-width: 1000px;
}
.heading {
  display: flex;
  justify-content: space-between;
  align-items: start;
  gap: var(--s-4);
  flex-wrap: wrap;
}
h1 {
  overflow-wrap: anywhere;
}
.heading p {
  margin-top: var(--s-2);
}
.actions {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}
.editor {
  display: grid;
  gap: var(--s-3);
  padding: var(--s-4);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
}
.body {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  line-height: 1.7;
}
.description {
  padding: var(--s-4) 0;
  border-bottom: 1px solid var(--border);
}
.discussion ol {
  list-style: none;
  margin: 0;
  padding: 0;
}
.discussion li {
  padding: var(--s-4) 0;
  border-bottom: 1px solid var(--border);
}
.discussion li .body {
  margin-top: var(--s-2);
}
.linked {
  padding: var(--s-3);
  background: var(--surface-2);
}
</style>
