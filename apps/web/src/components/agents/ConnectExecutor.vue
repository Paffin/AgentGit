<script setup lang="ts">
import { downloadTextFile } from '@/lib/download'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { randomHex } from '@/api/client'
import { ApiError, describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useResource } from '@/composables/useResource'
import { useServerNow } from '@/composables/useNow'
import {
  executorConfiguration,
  executorSetupProblem,
  EXECUTOR_OPERATIONS,
} from '@/lib/executorSetup'
import Checkbox from '@/ui/Checkbox.vue'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import TextField from '@/ui/TextField.vue'
import TextArea from '@/ui/TextArea.vue'
import SelectField from '@/ui/SelectField.vue'
import Alert from '@/ui/Alert.vue'
import CommandBlock from '@/components/CommandBlock.vue'

const props = defineProps<{ repo: string }>()
const auth = useAuth()
const open = ref(false),
  busy = ref(false),
  error = ref(''),
  issued = ref(false)
const captureOutput = ref(false)
const serverUrl = ref(window.location.origin)
const agent = ref('my-agent'),
  command = ref(''),
  args = ref('[]'),
  environment = ref(''),
  slots = ref('1')
const token = ref(''),
  credentialId = ref(''),
  mintedAt = ref(0),
  expiresAt = ref(0)
const now = useServerNow()
let context = 0,
  controller: AbortController | undefined
const canConnect = computed(() => auth.can(props.repo, 'connectAgent'))
const setupId = ref(randomHex().slice(0, 16))
const service = computed(() => `service:exec-${agent.value.slice(0, 40)}-${setupId.value}`)
const settings = computed(() => ({
  origin: serverUrl.value.trim().replace(/\/$/, ''),
  repo: props.repo,
  agent: agent.value,
  command: command.value,
  arguments: args.value,
  environment: environment.value,
  slots: Number(slots.value),
  captureOutput: captureOutput.value,
}))
const problem = computed(() => executorSetupProblem(settings.value))
const agents = useResource((signal) => api.agents.list(props.repo, { signal, token: auth.token }), {
  watch: [() => props.repo, () => auth.token, open, issued],
  enabled: () => open.value && issued.value,
})
const connection = computed(() => agents.data.value?.find((a) => a.name === agent.value)?.executor)
const connected = computed(
  () =>
    !!connection.value &&
    connection.value.reported_by === service.value &&
    connection.value.reported_at_ms >= mintedAt.value &&
    connection.value.expires_at_ms > now.value,
)
const timer = setInterval(() => {
  if (open.value && issued.value && document.visibilityState === 'visible') void agents.reload()
}, 3000)
watch(
  [() => props.repo, () => auth.token],
  () => {
    context++
    controller?.abort()
    token.value = ''
    credentialId.value = ''
    captureOutput.value = false
    issued.value = false
    open.value = false
    busy.value = false
    error.value = ''
    setupId.value = randomHex().slice(0, 16)
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
  controller?.abort()
  clearInterval(timer)
  token.value = ''
})

async function prepare() {
  if (busy.value || issued.value || !canConnect.value || problem.value) return
  const mine = context
  controller = new AbortController()
  const request = { token: auth.token, signal: controller.signal, quietUnauthenticated: true }
  const repo = props.repo,
    subject = service.value,
    name = agent.value
  busy.value = true
  error.value = ''
  let minting = false
  const description = `Executor setup ${name} (${randomHex()})`
  try {
    await api.admin.grantRole({ subject, repo, role: 'delegator' }, request)
    if (mine !== context) return
    await api.admin.grantRole({ subject: `agent:${name}`, repo, role: 'writer' }, request)
    if (mine !== context) return
    minting = true
    const result = await api.admin.mintCredential(
      {
        subject,
        description,
        ttl_secs: 86400,
        scope: { repos: [repo], operations: EXECUTOR_OPERATIONS },
      },
      request,
    )
    if (mine !== context) return
    token.value = result.token
    credentialId.value = result.credential.id
    mintedAt.value = result.credential.created_at_ms
    expiresAt.value = result.credential.expires_at_ms
    issued.value = true
    await agents.reload()
  } catch (cause) {
    if (mine === context) {
      const uncertain = minting && (!(cause instanceof ApiError) || cause.status === 0)
      error.value =
        describeError(cause) +
        (uncertain
          ? ` Token creation may have completed; revoke the credential named “${description}” in repository settings before retrying.`
          : '')
    }
  } finally {
    if (mine === context) busy.value = false
  }
}
function downloadConfig() {
  downloadTextFile(
    'agentgit-executor.json',
    executorConfiguration(settings.value),
    'application/json',
  )
}
function downloadToken() {
  if (token.value) downloadTextFile('agentgit-executor.token', token.value + '\n')
}
async function revoke() {
  if (busy.value || !credentialId.value) return
  const mine = context
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.admin.revokeCredential(credentialId.value, {
      token: auth.token,
      signal: controller.signal,
      quietUnauthenticated: true,
    })
    if (mine !== context) return
    await api.admin.revokeRole(
      { subject: service.value, repo: props.repo, role: 'delegator' },
      {
        token: auth.token,
        signal: controller.signal,
        quietUnauthenticated: true,
      },
    )
    if (mine !== context) return
    token.value = ''
    credentialId.value = ''
    issued.value = false
    setupId.value = randomHex().slice(0, 16)
  } catch (cause) {
    if (mine === context) error.value = describeError(cause)
  } finally {
    if (mine === context) busy.value = false
  }
}
</script>

<template>
  <Button v-if="canConnect" icon="plus" @click="open = true">Connect an executor…</Button>
  <Dialog v-model="open" title="Connect an executor" size="lg" :dismissable="!busy">
    <div class="setup">
      <p>
        Choose an installed agent program. This page prepares repository access and its
        configuration. Start the executor once on your machine or server; then assign work here.
      </p>
      <fieldset v-if="!issued" :disabled="busy">
        <TextField
          v-model="serverUrl"
          label="AgentGit URL reachable from the executor"
          hint="Must point to this installation. Use HTTPS for a remote host."
        />
        <TextField v-model="agent" label="Agent name" placeholder="my-agent" />
        <TextField
          v-model="command"
          label="Executable on the executor host"
          placeholder="/opt/agents/coding-agent"
          hint="The program receives the task and a run-bound credential through AG_TASK, AG_REPO, AG_RUN and AG_TOKEN. It must implement the AgentGit executor contract."
        />
        <TextArea v-model="args" label="Arguments (JSON array)" :rows="2" />
        <TextField
          v-model="environment"
          label="Pass environment variables (names only)"
          placeholder="MODEL_API_KEY"
          hint="Values stay on the executor host. PATH is always included."
        />
        <SelectField
          v-model="slots"
          label="Concurrent tasks"
          :options="[1, 2, 4, 8].map((n) => ({ value: String(n), label: String(n) }))"
        />
        <Checkbox
          v-if="auth.capabilities?.features.includes('agents.run_output')"
          v-model="captureOutput"
          label="Save diagnostic output in AgentGit"
        />
        <p v-if="captureOutput" class="faint">
          Saves the last 32 KiB of stdout and stderr when the run ends. Known credentials are
          redacted, but output may contain other private data. Repository readers can view it.
        </p>
      </fieldset>
      <dl v-else class="summary">
        <dt>Agent</dt>
        <dd>{{ agent }}</dd>
        <dt>Executable</dt>
        <dd>
          <code>{{ command }}</code>
        </dd>
        <dt>Concurrent tasks</dt>
        <dd>{{ slots }}</dd>
      </dl>
      <p>
        <RouterLink :to="`/repos/${repo}/admin?tab=credentials`"
          >Manage repository credentials</RouterLink
        >
        · The executor can propose changes and manage its agent runs. It cannot approve, integrate
        or change repository policy.
      </p>
      <Alert v-if="error" tone="bad" live="alert" title="Could not complete setup">{{
        error
      }}</Alert>
      <template v-if="!issued">
        <p v-if="problem" class="faint">{{ problem }}</p>
        <Button
          variant="primary"
          :disabled="!!problem || !canConnect"
          :loading="busy"
          @click="prepare"
          >Prepare access for 24 hours</Button
        >
      </template>
      <template v-else>
        <Alert
          :tone="connected ? 'ok' : 'info'"
          :title="connected ? 'Executor connected' : 'Waiting for the executor'"
        >
          {{
            connected
              ? 'You can now assign tasks to this agent. A connection does not guarantee a successful task.'
              : 'Download both files into the same directory on the executor host and run the command below.'
          }}
        </Alert>
        <p>
          Access expires {{ new Date(expiresAt).toLocaleString() }}. The token is shown only in this
          setup session; downloaded copies remain valid until expiry or revocation.
        </p>
        <div class="actions">
          <Button @click="downloadConfig">Download configuration</Button>
          <Button :disabled="!token" @click="downloadToken">Download private token</Button>
          <Button :disabled="!token" @click="token = ''">Forget token on this page</Button>
        </div>
        <CommandBlock
          label="Start the executor on its host"
          :lines="[
            'chmod 600 agentgit-executor.token',
            'orchestrator run -config agentgit-executor.json -token-file agentgit-executor.token',
          ]"
        />
        <p class="faint">
          Keep the token private. Install the orchestrator on the host first. Granting access does
          not install software or launch a model from this browser.
        </p>
        <Button variant="danger" :loading="busy" @click="revoke">Revoke this executor token</Button>
      </template>
    </div>
  </Dialog>
</template>

<style scoped>
.setup,
fieldset {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  min-width: 0;
}
fieldset {
  border: 0;
  margin: 0;
  padding: 0;
}
.summary {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: var(--s-2) var(--s-4);
}
.summary dt {
  color: var(--text-3);
}
.summary dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.actions {
  display: flex;
  gap: var(--s-2);
  flex-wrap: wrap;
}
</style>
