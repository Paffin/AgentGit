<script setup lang="ts">
import { computed, ref, onBeforeUnmount } from 'vue'
import { useServerNow } from '@/composables/useNow'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Agent } from '@/api/types'
import { useLiveReload } from '@/composables/useLiveReload'
import { useResource } from '@/composables/useResource'
import { shortId } from '@/lib/format'
import { requirementText } from '@/lib/roles'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import DataTable from '@/ui/DataTable.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import SelectField from '@/ui/SelectField.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const auth = useAuth()
const toasts = useToasts()

const agents = useResource<Agent[]>(() => api.agents.list(props.repo), {
  watch: [() => props.repo, () => auth.token],
})
useLiveReload(['agent'], () => agents.reload())
const now = useServerNow()
const hasPresence = computed(
  () => !!auth.capabilities?.features.includes('agents.executor_presence'),
)
const connected = (agent: Agent) =>
  !agent.disabled && !!agent.executor && now.value < agent.executor.expires_at_ms
const refresh = setInterval(() => {
  if (hasPresence.value && document.visibilityState === 'visible') void agents.reload()
}, 20_000)
onBeforeUnmount(() => clearInterval(refresh))

const columns = computed<Column[]>(() =>
  (
    [
      { key: 'identity', label: 'Agent', width: 'minmax(0, 1.6fr)' },
      { key: 'executor', label: 'Executor connection', width: 'minmax(0, 1.4fr)', hideBelow: 800 },
      { key: 'role', label: 'Role', width: '7rem', hideBelow: 600 },
      { key: 'status', label: 'Status', width: '7rem' },
      { key: 'public_key', label: 'Public key', width: 'minmax(0, 1.4fr)', hideBelow: 900 },
      { key: 'created_at_ms', label: 'Registered', width: '8rem', align: 'end', hideBelow: 600 },
    ] satisfies Column[]
  ).filter((column) => column.key !== 'executor' || hasPresence.value),
)

const registering = ref(false)
const name = ref('')
const key = ref('')
const role = ref('agent')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const canRegister = computed(() => auth.can(props.repo, 'registerAgent'))
const keyProblem = computed(() =>
  key.value && !/^[0-9a-f]{64}$/.test(key.value.trim())
    ? 'An Ed25519 public key is 64 lowercase hex characters.'
    : '',
)

function open() {
  name.value = ''
  key.value = ''
  role.value = 'agent'
  error.value = null
  registering.value = true
}
async function register() {
  busy.value = true
  error.value = null
  try {
    await api.agents.register(props.repo, {
      name: name.value.trim(),
      public_key: key.value.trim(),
      role: role.value,
    })
    registering.value = false
    toasts.ok('Agent registered', name.value.trim())
    await agents.reload()
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="agents">
    <div class="bar">
      <p class="muted">
        Agents are long-lived principals with an Ed25519 key; each execution is a separate run with
        its own identity. A registered key does not start an executor.
      </p>
      <Button
        icon="plus"
        :disabled="!canRegister"
        :disabled-reason="requirementText('registerAgent')"
        @click="open"
        >Register agent</Button
      >
    </div>
    <p v-if="hasPresence" class="faint">
      Connection is reported by a running orchestrator and expires without renewal. It does not
      confirm that a model, credentials or task command will succeed.
    </p>
    <ErrorState
      v-if="agents.error.value && !agents.data.value"
      what="agents"
      :error="agents.error.value"
      @retry="agents.reload()"
    />
    <DataTable
      v-else
      :columns="columns"
      :rows="agents.data.value ?? []"
      :row-key="(a: Agent) => a.name"
      caption="Registered agents"
      :loading="!agents.settled.value"
    >
      <template #cell-identity="{ row }"
        ><IdentityLabel :identity="(row as Agent).identity" :size="22" />
        <span v-if="hasPresence" class="executor-mobile">
          {{ connected(row as Agent) ? 'Executor connected' : 'No recent executor report' }}
        </span>
      </template>
      <template #cell-executor="{ row }">
        <div class="executor">
          <Badge :tone="connected(row as Agent) ? 'ok' : 'neutral'" size="sm">
            {{ connected(row as Agent) ? 'Connected' : 'No recent report' }}
          </Badge>
          <template v-if="(row as Agent).executor">
            <span>{{ (row as Agent).executor!.name }}</span>
            <span v-if="connected(row as Agent)" class="faint"
              >{{ (row as Agent).executor!.available }} / {{ (row as Agent).executor!.slots }} slots
              free at last report</span
            >
            <span class="faint"
              >Last contact <RelativeTime :ms="(row as Agent).executor!.reported_at_ms"
            /></span>
          </template>
        </div>
      </template>
      <template #cell-status="{ row }"
        ><Badge size="sm" :tone="(row as Agent).disabled ? 'bad' : 'ok'" dot>{{
          (row as Agent).disabled ? 'disabled' : 'enabled'
        }}</Badge></template
      >
      <template #cell-public_key="{ row }"
        ><code class="mono">{{ shortId((row as Agent).public_key, 20) }}</code></template
      >
      <template #cell-created_at_ms="{ row }"
        ><span class="faint"><RelativeTime :ms="(row as Agent).created_at_ms" /></span
      ></template>
      <template #empty
        ><EmptyState
          icon="bot"
          compact
          title="No agents registered"
          description="Register an agent’s public key so its provenance can be verified."
      /></template>
    </DataTable>

    <Dialog
      v-model="registering"
      title="Register an agent"
      description="Only the public key is sent. The private key must stay with the agent."
      :dismissable="!busy"
    >
      <form id="reg-agent" class="form" @submit.prevent="register">
        <TextField
          v-model="name"
          label="Name"
          mono
          required
          autofocus
          placeholder="refactor-bot"
          hint="The identity becomes agent:<name>."
        />
        <TextField
          v-model="key"
          label="Public key (hex)"
          mono
          required
          :error="keyProblem"
          placeholder="64 hex characters"
        />
        <SelectField
          v-model="role"
          label="Role"
          :options="[
            { value: 'agent', label: 'agent' },
            { value: 'reviewer', label: 'reviewer' },
          ]"
        />
        <Alert v-if="error" tone="bad" live="alert" title="Could not register the agent">{{
          error.message
        }}</Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="registering = false">Cancel</Button>
        <Button
          type="submit"
          form="reg-agent"
          variant="primary"
          :loading="busy"
          :disabled="!!keyProblem"
          >Register</Button
        >
      </template>
    </Dialog>
  </div>
</template>

<style scoped>
.agents {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.executor {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  overflow-wrap: anywhere;
}
.executor-mobile {
  display: none;
  font-size: var(--fs-xs);
  color: var(--text-3);
}
@media (max-width: 799px) {
  .executor-mobile {
    display: block;
    margin-top: 4px;
  }
}
</style>
