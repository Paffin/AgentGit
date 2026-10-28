<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { Ref } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { RoleBinding } from '@/api/types'
import { ROLES } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { repoPatternMatches } from '@/lib/glob'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import DataTable from '@/ui/DataTable.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import SelectField from '@/ui/SelectField.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const props = defineProps<{ repo?: string }>()
const toasts = useToasts()
const auth = useAuth()

const bindings = useResource<RoleBinding[]>(() => api.admin.roles(), { watch: [() => props.repo] })
const rows = computed(() => {
  const all = bindings.data.value ?? []
  return props.repo ? all.filter((b) => repoPatternMatches(b.repo, props.repo as string)) : all
})

const columns: Column[] = [
  { key: 'subject', label: 'Who', width: 'minmax(0, 1.4fr)' },
  { key: 'repo', label: 'Repositories', width: 'minmax(0, 1fr)' },
  { key: 'role', label: 'Role', width: '8rem' },
  { key: 'created', label: 'Granted', width: 'minmax(0, 1.2fr)', hideBelow: 900 },
  { key: 'actions', label: 'Actions', width: '5rem', align: 'end', srOnlyHeader: true },
]

const granting = ref(false)
const subject = ref('')
const repoPattern = ref('')
const role = ref('reader')
const busy = ref(false)
const error = ref<ApiError | null>(null)

let context = 0
watch(
  [() => props.repo, () => auth.identity, () => auth.status],
  () => {
    context++
    granting.value = false
    busy.value = false
    error.value = null
    revoking.value = null
    revokeBusy.value = false
  },
  { flush: 'sync' },
)
onBeforeUnmount(() => {
  context++
})

function openGrant() {
  if (busy.value) return
  subject.value = ''
  repoPattern.value = props.repo ?? ''
  role.value = 'reader'
  error.value = null
  granting.value = true
}
function reportError(target: Ref<ApiError | null>, mine: number, cause: unknown) {
  if (mine !== context) return
  target.value =
    cause instanceof ApiError
      ? cause
      : new ApiError({ status: 0, code: 'internal', message: describeError(cause) })
}

async function grant() {
  if (busy.value || auth.status !== 'ready' || error.value?.status === 401) return
  const mine = context
  const binding = {
    subject: subject.value.trim(),
    repo: repoPattern.value.trim(),
    role: role.value,
  }
  const options = { token: auth.token, quietUnauthenticated: true }
  busy.value = true
  error.value = null
  try {
    await api.admin.grantRole(binding, options)
    if (mine !== context) return
    granting.value = false
    toasts.ok('Role granted', `${binding.subject} is now ${binding.role} on ${binding.repo}`)
    await bindings.reload()
  } catch (e) {
    reportError(error, mine, e)
  } finally {
    if (mine === context) busy.value = false
  }
}

const revoking = ref<RoleBinding | null>(null)
const revokeError = ref<ApiError | null>(null)
watch(revoking, () => {
  revokeError.value = null
})
const revokeOpen = computed({
  get: () => revoking.value !== null,
  set: (v: boolean) => {
    if (!v) revoking.value = null
  },
})
const revokeBusy = ref(false)
async function revoke() {
  const b = revoking.value
  if (!b || revokeBusy.value || auth.status !== 'ready' || revokeError.value?.status === 401) return
  const mine = context
  const binding = { subject: b.subject, repo: b.repo, role: b.role }
  const options = { token: auth.token, quietUnauthenticated: true }
  revokeBusy.value = true
  revokeError.value = null
  try {
    await api.admin.revokeRole(binding, options)
    if (mine !== context) return
    toasts.ok('Role revoked', `${binding.subject} no longer has ${binding.role} on ${binding.repo}`)
    revoking.value = null
    await bindings.reload()
  } catch (e) {
    reportError(revokeError, mine, e)
  } finally {
    if (mine === context) revokeBusy.value = false
  }
}
const roleOptions = ROLES.map((r) => ({ value: r, label: r }))
</script>

<template>
  <Card
    title="Role bindings"
    icon="shield"
    :subtitle="repo ? `Bindings that apply to ${repo}` : 'Who may do what, on which repositories'"
    flush
  >
    <template #actions>
      <Button
        size="sm"
        icon="plus"
        variant="primary"
        data-testid="grant-open"
        :disabled="busy"
        @click="openGrant"
        >Grant a role</Button
      >
    </template>
    <ErrorState
      v-if="bindings.error.value && !bindings.data.value"
      what="role bindings"
      :error="bindings.error.value"
      compact
      @retry="bindings.reload()"
    />
    <DataTable
      v-else
      :columns="columns"
      :rows="rows"
      :row-key="(b: RoleBinding) => `${b.subject}|${b.repo}|${b.role}`"
      caption="Role bindings"
      :loading="!bindings.settled.value"
      dense
    >
      <template #cell-subject="{ row }"
        ><IdentityLabel :identity="(row as RoleBinding).subject" :size="18"
      /></template>
      <template #cell-repo="{ row }"
        ><code class="mono">{{ (row as RoleBinding).repo }}</code></template
      >
      <template #cell-role="{ row }"
        ><Badge :tone="(row as RoleBinding).role === 'admin' ? 'warn' : 'accent'" size="sm">{{
          (row as RoleBinding).role
        }}</Badge></template
      >
      <template #cell-created="{ row }"
        ><span class="faint"
          >{{ (row as RoleBinding).created_by }} ·
          <RelativeTime :ms="(row as RoleBinding).created_at_ms" /></span
      ></template>
      <template #cell-actions="{ row }"
        ><Button
          size="sm"
          variant="ghost"
          icon="trash"
          label="Revoke role"
          @click="revoking = row as RoleBinding"
      /></template>
      <template #empty
        ><EmptyState
          icon="shield"
          compact
          title="No bindings"
          description="Without a role, a user cannot even see that a repository exists."
      /></template>
    </DataTable>

    <Dialog
      v-model="granting"
      title="Grant a role"
      :dismissable="!busy"
      description="Subjects and repositories accept patterns such as agent:* or acme/*."
    >
      <form id="grant-form" class="form" @submit.prevent="grant">
        <TextField
          v-model="subject"
          :disabled="busy"
          label="Who"
          mono
          required
          autofocus
          placeholder="user:alice"
          hint="user:alice, agent:*, service:ci, or group:platform."
        />
        <TextField
          v-model="repoPattern"
          :disabled="busy"
          label="Repository"
          mono
          required
          placeholder="acme/webapp"
          hint="A repository, a pattern like acme/*, or * for all."
        />
        <SelectField
          v-model="role"
          :disabled="busy"
          label="Role"
          :options="roleOptions"
          hint="reader · writer · reviewer · integrator · runner · delegator · admin"
        />
        <Alert
          v-if="error"
          tone="bad"
          live="alert"
          :title="error.status === 401 ? 'Sign in again' : 'Could not grant the role'"
        >
          <template v-if="error.status === 401">
            Your session was rejected. Sign in again before granting a role.
            <Button to="/signin">Sign in</Button>
          </template>
          <template v-else>{{ error.message }}</template>
        </Alert>
      </form>
      <template #footer>
        <Button :disabled="busy" @click="granting = false">Cancel</Button>
        <Button
          type="submit"
          form="grant-form"
          variant="primary"
          :loading="busy"
          :disabled="error?.status === 401"
          >Grant</Button
        >
      </template>
    </Dialog>

    <Dialog
      v-model="revokeOpen"
      title="Revoke this role?"
      alert
      size="sm"
      :dismissable="!revokeBusy"
    >
      <p v-if="revoking">
        <strong>{{ revoking.subject }}</strong> will lose <strong>{{ revoking.role }}</strong> on
        <code class="mono">{{ revoking.repo }}</code
        >.
      </p>
      <Alert v-if="revokeError" tone="bad" live="alert" title="Could not revoke the role">
        <template v-if="revokeError.status === 401">
          Your session was rejected. Sign in again before revoking a role.
          <Button to="/signin">Sign in</Button>
        </template>
        <template v-else>{{ revokeError.message }}</template>
      </Alert>
      <template #footer>
        <Button :disabled="revokeBusy" @click="revoking = null">Keep it</Button>
        <Button
          variant="danger"
          :loading="revokeBusy"
          :disabled="revokeError?.status === 401"
          @click="revoke"
          >Revoke</Button
        >
      </template>
    </Dialog>
  </Card>
</template>

<style scoped>
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
</style>
