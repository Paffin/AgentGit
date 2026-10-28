<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { randomHex } from '@/api/client'
import { ApiError, describeError } from '@/api/errors'
import type { Task } from '@/api/types'
import CommandBlock from '@/components/CommandBlock.vue'
import ConnectAgentPanel from '@/components/ConnectAgentPanel.vue'
import { agentWorkLines, matchesAgentTask, subjectProblem } from '@/lib/onboarding'
import { requirementText } from '@/lib/roles'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import CopyButton from '@/ui/CopyButton.vue'
import Icon from '@/ui/Icon.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'

/**
 * "Give an agent work": describe the task, connect the agent (the page grants the role and mints the credential
 * itself), and watch for the agent to start. The progress list ticks from what the board already knows.
 */
const props = defineProps<{ repo: string }>()
const auth = useAuth()
const board = useBoard()
const toasts = useToasts()

const canTask = computed(() => auth.can(props.repo, 'createTask'))
const canConnect = computed(() => auth.can(props.repo, 'connectAgent'))

// ---- 1. the task -----------------------------------------------------------------------------------------
const title = ref('')
const details = ref('')
const scope = ref('')
const task = ref<Task | null>(null)
const taskBusy = ref(false)
const taskError = ref<ApiError | null>(null)

let context = 0
let controller: AbortController | undefined
let taskKey = randomHex()
watch(
  [title, details, scope],
  () => {
    taskKey = randomHex()
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
})

const asError = (e: unknown) =>
  e instanceof ApiError
    ? e
    : new ApiError({ status: 0, code: 'internal', message: describeError(e) })

async function createTask() {
  if (taskBusy.value || !canTask.value || !title.value.trim()) return
  const mine = context
  controller = new AbortController()
  const request = {
    token: auth.token,
    signal: controller.signal,
    idempotencyKey: taskKey,
    quietUnauthenticated: true,
  }
  taskBusy.value = true
  taskError.value = null
  try {
    const created = await api.agents.createTask(
      props.repo,
      {
        title: title.value.trim(),
        description: details.value.trim() || undefined,
        scope: scope.value
          .split(/[\n,]+/)
          .map((s) => s.trim())
          .filter(Boolean),
      },
      request,
    )
    if (mine !== context) return
    task.value = created
    board.touched(['tasks'])
    toasts.ok('Task created', created.title)
  } catch (e) {
    if (mine === context) taskError.value = asError(e)
  } finally {
    if (mine === context) taskBusy.value = false
  }
}

// ---- 2. the agent ----------------------------------------------------------------------------------------
const agentName = ref('my-agent')
const subject = computed(() => `agent:${agentName.value.trim()}`)
const subjectError = computed(() => subjectProblem(subject.value))
const token = ref('')
const connectBusy = ref(false)
const connectError = ref<ApiError | null>(null)
const connected = ref('')

watch(
  [() => props.repo, () => auth.token],
  () => {
    context++
    controller?.abort()
    title.value = ''
    details.value = ''
    scope.value = ''
    agentName.value = 'my-agent'
    taskKey = randomHex()
    token.value = ''
    connected.value = ''
    task.value = null
    taskBusy.value = false
    connectBusy.value = false
    taskError.value = null
    connectError.value = null
  },
  { flush: 'sync' },
)

async function connect() {
  if (connectBusy.value || !canConnect.value || !task.value || subjectError.value) return
  const mine = context
  const repository = props.repo
  const principal = subject.value
  const taskTitle = task.value.title
  controller = new AbortController()
  const request = { token: auth.token, signal: controller.signal, quietUnauthenticated: true }
  connectBusy.value = true
  connectError.value = null
  try {
    await api.admin.grantRole({ subject: principal, repo: repository, role: 'writer' }, request)
    if (mine !== context) return
    const minted = await api.admin.mintCredential(
      {
        subject: principal,
        description: `Connected from the web UI for ${taskTitle}`,
        scope: { repos: [repository] },
        ttl_secs: 3600,
      },
      request,
    )
    if (mine !== context) return
    token.value = minted.token
    connected.value = principal
    toasts.ok('Agent connected', `${principal} may write to ${repository} for an hour.`)
  } catch (e) {
    if (mine === context) connectError.value = asError(e)
  } finally {
    if (mine === context) connectBusy.value = false
  }
}

function forget() {
  token.value = ''
}

const lines = computed(() =>
  token.value
    ? agentWorkLines({
        origin: window.location.origin,
        repo: props.repo,
        token: token.value,
        title: task.value?.title ?? title.value,
        taskId: task.value?.id,
      })
    : null,
)
const prompt = computed(() =>
  task.value
    ? `Your task (${task.value.id}): ${task.value.title}.${task.value.description ? ` ${task.value.description}` : ''}${
        task.value.scope.length ? ` Only touch: ${task.value.scope.join(', ')}.` : ''
      } Work in a clone of ${props.repo}, snapshot your changes and push them as a change request linked to task ${task.value.id}.`
    : '',
)

// ---- 3. watching -----------------------------------------------------------------------------------------
const agentRun = computed(() =>
  connected.value
    ? board.runs
        .filter((r) =>
          matchesAgentTask(task.value?.id ?? '', connected.value, r.task_id, r.identity),
        )
        .sort((a, b) => b.started_at_ms - a.started_at_ms)[0]
    : undefined,
)
const agentProposal = computed(() =>
  connected.value
    ? board.cards.find((c) =>
        matchesAgentTask(
          task.value?.id ?? '',
          connected.value,
          c.proposal.task_id,
          c.proposal.author,
        ),
      )
    : undefined,
)
const steps = computed(() => [
  { id: 'task', label: 'Task described', done: !!task.value },
  { id: 'agent', label: 'Agent connected', done: !!connected.value },
  {
    id: 'run',
    label: agentRun.value
      ? agentRun.value.status === 'running'
        ? 'Agent started working'
        : 'Agent session ended'
      : agentProposal.value
        ? 'Agent submitted changes'
        : 'Waiting for the agent to start',
    done: !!agentRun.value || !!agentProposal.value,
  },
  { id: 'cr', label: 'Change request opened', done: !!agentProposal.value },
])
const current = computed(() => steps.value.find((s) => !s.done)?.id ?? 'cr')
const repoBase = computed(() => `/repos/${props.repo}`)
</script>

<template>
  <Card
    title="Give an agent work"
    icon="bot"
    subtitle="Describe the work, connect an agent, and watch for its change request."
    data-testid="give-work"
  >
    <ol class="progress" aria-label="Progress" data-testid="give-work-progress">
      <li v-for="s in steps" :key="s.id" :class="{ done: s.done, now: s.id === current }">
        <span class="mark" aria-hidden="true"
          ><Icon :name="s.done ? 'check' : 'circle'" :size="12"
        /></span>
        {{ s.label }}<span class="sr-only">{{ s.done ? ' (done)' : '' }}</span>
      </li>
    </ol>

    <section class="step" aria-labelledby="gw-1">
      <h3 id="gw-1"><span class="n" aria-hidden="true">1</span> Describe the work</h3>
      <template v-if="!task">
        <form class="form" @submit.prevent="createTask">
          <TextField
            v-model="title"
            label="What should the agent do?"
            required
            placeholder="Fix the flaky invoice test"
            :disabled="!canTask"
            hint="One sentence. It becomes the task’s title."
          />
          <TextArea
            v-model="details"
            label="Details (optional)"
            :rows="2"
            :disabled="!canTask"
            placeholder="What is wrong, what good looks like"
          />
          <TextField
            v-model="scope"
            label="Files it may touch (optional)"
            mono
            :disabled="!canTask"
            placeholder="src/billing/**"
            hint="Paths, separated by commas. Leave empty if it does not matter."
          />
          <Alert v-if="taskError" tone="bad" live="alert" title="Could not create the task">{{
            taskError.message
          }}</Alert>
          <div>
            <Button
              type="submit"
              variant="primary"
              :loading="taskBusy"
              :disabled="!canTask || !title.trim()"
              :disabled-reason="requirementText('createTask')"
              data-testid="create-task"
              >Create task</Button
            >
          </div>
          <p v-if="!canTask" class="faint">{{ requirementText('createTask') }}.</p>
        </form>
      </template>
      <p v-else class="done-line" data-testid="task-made">
        <Icon name="check" :size="14" /> Task “{{ task.title }}” is ready.
      </p>
    </section>

    <section v-if="task" class="step" aria-labelledby="gw-2">
      <h3 id="gw-2"><span class="n" aria-hidden="true">2</span> Connect an agent</h3>
      <template v-if="!canConnect">
        <p class="faint">
          Connecting an agent gives it a credential, which needs the admin role. Send this page to
          an administrator, or give the agent a credential from the command line (below).
        </p>
      </template>
      <template v-else-if="!token">
        <form class="form" @submit.prevent="connect">
          <TextField
            v-model="agentName"
            label="Agent name"
            mono
            :error="subjectError ?? undefined"
            hint="Becomes agent:<name> in the history. Letters, digits, dot, dash, underscore."
          />
          <p class="faint">
            This lets <code>{{ subject }}</code> write to {{ repo }} for one hour, and nothing else.
          </p>
          <Alert v-if="connectError" tone="bad" live="alert" title="Could not connect the agent">{{
            connectError.message
          }}</Alert>
          <div>
            <Button
              type="submit"
              variant="primary"
              :loading="connectBusy"
              :disabled="!!subjectError"
              data-testid="connect-agent-go"
              >Create the agent’s credential</Button
            >
          </div>
        </form>
      </template>
      <div v-else class="token" data-testid="agent-token-box">
        <Alert tone="warn" title="Copy this now: it is shown once">
          The credential is in the commands below. The server keeps only a hash, so it cannot show
          it again. Closing this page or pressing “Hide it” removes it from the screen.
        </Alert>
        <div class="row">
          <h4>Run this where the agent works</h4>
          <CopyButton v-if="lines" :text="lines.join('\n')" label="Copy the commands" with-label />
        </div>
        <CommandBlock
          v-if="lines"
          :lines="lines"
          label="Commands for the agent"
          testid="agent-commands"
        />
        <p v-else class="faint">
          The address or the credential could not be put into a shell line safely.
        </p>
        <div class="row">
          <h4>Or tell the agent in words</h4>
          <CopyButton :text="prompt" label="Copy the instruction" with-label />
        </div>
        <p class="prompt" data-testid="agent-prompt">{{ prompt }}</p>
        <Button size="sm" variant="ghost" icon="eye-off" @click="forget">Hide it</Button>
      </div>
    </section>

    <section v-if="connected" class="step" aria-labelledby="gw-3">
      <h3 id="gw-3"><span class="n" aria-hidden="true">3</span> Watch it work</h3>
      <p v-if="agentProposal" data-testid="agent-proposed">
        <Icon name="check" :size="14" />
        {{ connected }} opened
        <RouterLink
          :to="`${repoBase}/proposals/${encodeURIComponent(agentProposal.proposal.id)}`"
          >{{ agentProposal.proposal.title }}</RouterLink
        >. Review it next.
      </p>
      <p v-else-if="agentRun?.status === 'running'" class="faint" data-testid="agent-working">
        {{ connected }} is working. Its change request appears here and under Change requests.
      </p>
      <p v-else-if="agentRun" class="faint" data-testid="agent-ended">
        The agent session ended ({{ agentRun.status }}) without a linked change request. Open the
        task to inspect its runs.
      </p>
      <p v-else class="faint" data-testid="agent-waiting">
        No run or change request is linked to this task yet. Sending a change with the commands
        above links it automatically.
      </p>
    </section>

    <details class="alt">
      <summary>Prefer the command line only?</summary>
      <ConnectAgentPanel :repo="repo" :admin="canConnect" />
    </details>
  </Card>
</template>

<style scoped>
.progress {
  list-style: none;
  margin: 0 0 var(--s-4);
  padding: 0;
  display: flex;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.progress li {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.progress .now {
  color: var(--text);
  font-weight: 600;
}
.mark {
  display: inline-grid;
  place-items: center;
  width: 1.25rem;
  height: 1.25rem;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
}
.progress .done .mark {
  background: var(--ok-soft, var(--surface-2));
  border-color: var(--ok-line, var(--ok));
  color: var(--ok-text, var(--ok));
}
.step {
  padding-top: var(--s-3);
  border-top: 1px solid var(--border);
  margin-top: var(--s-3);
}
.step h3 {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-md, 1rem);
  margin: 0 0 var(--s-3);
}
.n {
  display: inline-grid;
  place-items: center;
  width: 1.5rem;
  height: 1.5rem;
  border-radius: 50%;
  background: var(--accent-soft, var(--surface-2));
  color: var(--accent-text, var(--text));
  font-size: 0.8rem;
}
.form,
.token {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  min-width: 0;
}
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.row h4 {
  margin: 0;
}
.prompt {
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: var(--s-3);
  overflow-wrap: anywhere;
  user-select: all;
}
.done-line {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--ok-text, var(--text));
}
.alt {
  margin-top: var(--s-4);
  border-top: 1px solid var(--border);
  padding-top: var(--s-3);
}
.alt summary {
  cursor: pointer;
  font-weight: 560;
  min-height: var(--touch-min);
  display: flex;
  align-items: center;
}
</style>
