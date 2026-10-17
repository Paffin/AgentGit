<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import type { Credential } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { useServerNow } from '@/composables/useNow'
import { formatDateTime, shortId } from '@/lib/format'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import Checkbox from '@/ui/Checkbox.vue'
import CopyButton from '@/ui/CopyButton.vue'
import DataTable from '@/ui/DataTable.vue'
import Dialog from '@/ui/Dialog.vue'
import EmptyState from '@/ui/EmptyState.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import SelectField from '@/ui/SelectField.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'
import type { Column } from '@/ui/types'

const props = withDefaults(defineProps<{ defaultRepos?: string[]; title?: string }>(), {
  defaultRepos: () => [],
  title: 'Credentials',
})
const auth = useAuth()
const toasts = useToasts()
const now = useServerNow()

const subjectFilter = ref('')
const includeRevoked = ref(false)
const list = useResource<Credential[]>(
  () =>
    api.admin.credentials({
      subject: subjectFilter.value.trim() || undefined,
      include_revoked: includeRevoked.value || undefined,
    }),
  { watch: [subjectFilter, includeRevoked] },
)

const columns: Column[] = [
  { key: 'subject', label: 'Subject', width: 'minmax(0, 1.3fr)' },
  { key: 'description', label: 'Description', width: 'minmax(0, 1.4fr)', hideBelow: 800 },
  { key: 'scope', label: 'Scope', width: 'minmax(0, 1.4fr)', hideBelow: 1000 },
  { key: 'expires_at_ms', label: 'Expires', width: '9rem' },
  { key: 'last_used_ms', label: 'Last used', width: '8rem', hideBelow: 900 },
  { key: 'actions', label: 'Actions', width: '6.5rem', align: 'end', srOnlyHeader: true },
]

const stateOf = (c: Credential) =>
  c.revoked_at_ms
    ? 'revoked'
    : c.expires_at_ms && c.expires_at_ms <= now.value
      ? 'expired'
      : 'active'

// ---- mint -------------------------------------------------------------------------------------------
const TTLS = [
  { value: '900', label: '15 minutes' },
  { value: '3600', label: '1 hour' },
  { value: '86400', label: '1 day' },
  { value: String(7 * 86400), label: '7 days' },
  { value: String(30 * 86400), label: '30 days' },
]
const minting = ref(false)
const form = ref({
  subject: '',
  description: '',
  ttl: '3600',
  repos: '',
  streams: '',
  operations: '',
})
const busy = ref(false)
const error = ref<ApiError | null>(null)
// The secret lives only in this component, only while the dialog shows it.
const minted = ref<{ token: string; credential: Credential } | null>(null)
const revealed = ref(true)

const isAdmin = computed(() => auth.isAdminAnywhere)

function openMint() {
  form.value = {
    subject: '',
    description: '',
    ttl: '3600',
    repos: props.defaultRepos.join(', '),
    streams: '',
    operations: '',
  }
  error.value = null
  minted.value = null
  revealed.value = true
  minting.value = true
}
const split = (s: string) =>
  s
    .split(/[\s,]+/)
    .map((x) => x.trim())
    .filter(Boolean)

async function mint() {
  busy.value = true
  error.value = null
  try {
    const res = await api.admin.mintCredential({
      subject: form.value.subject.trim() || undefined,
      description: form.value.description.trim() || undefined,
      ttl_secs: Number(form.value.ttl),
      scope: {
        repos: split(form.value.repos),
        streams: split(form.value.streams),
        operations: split(form.value.operations),
      },
    })
    minted.value = { token: res.token, credential: res.credential }
    void list.reload()
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}

// Closing the dialog throws the token away.
watch(minting, (open) => {
  if (!open) {
    minted.value = null
    revealed.value = true
  }
})

// ---- revoke -----------------------------------------------------------------------------------------
const revoking = ref<Credential | null>(null)
const revokeBusy = ref(false)
const revokeOpen = computed({
  get: () => revoking.value !== null,
  set: (v: boolean) => {
    if (!v) revoking.value = null
  },
})
async function revoke() {
  const c = revoking.value
  if (!c) return
  revokeBusy.value = true
  try {
    await api.admin.revokeCredential(c.id)
    toasts.ok('Credential revoked', 'It stops working immediately.')
    revoking.value = null
    await list.reload()
  } catch (e) {
    toasts.error('Could not revoke the credential', e)
  } finally {
    revokeBusy.value = false
  }
}
</script>

<template>
  <Card :title="title" icon="key" flush>
    <template #actions>
      <Button variant="primary" size="sm" icon="plus" data-testid="mint-open" @click="openMint"
        >Mint a token</Button
      >
    </template>
    <div class="tools">
      <TextField
        v-if="isAdmin"
        v-model="subjectFilter"
        label="Subject"
        placeholder="Yours by default; admins can look up e.g. user:alice"
        mono
        class="subj"
      />
      <Checkbox v-model="includeRevoked" label="Include revoked" />
    </div>
    <ErrorState
      v-if="list.error.value && !list.data.value"
      what="credentials"
      :error="list.error.value"
      compact
      @retry="list.reload()"
    />
    <DataTable
      v-else
      :columns="columns"
      :rows="list.data.value ?? []"
      :row-key="(c: Credential) => c.id"
      caption="Credentials"
      :loading="!list.settled.value"
      dense
    >
      <template #cell-subject="{ row }">
        <IdentityLabel :identity="(row as Credential).subject" :size="18" />
        <p class="faint mono id">
          {{ shortId((row as Credential).id, 14) }}
          <Badge
            v-if="stateOf(row as Credential) !== 'active'"
            size="sm"
            :tone="stateOf(row as Credential) === 'revoked' ? 'bad' : 'warn'"
            >{{ stateOf(row as Credential) }}</Badge
          >
        </p>
      </template>
      <template #cell-description="{ row }"
        ><span class="muted">{{ (row as Credential).description || '–' }}</span></template
      >
      <template #cell-scope="{ row }">
        <span
          v-if="
            !(row as Credential).scope ||
            (row as Credential).scope!.repos.length +
              (row as Credential).scope!.streams.length +
              (row as Credential).scope!.operations.length ===
              0
          "
          class="faint"
          >unrestricted</span
        >
        <span v-else class="chips">
          <Badge v-for="r in (row as Credential).scope!.repos" :key="'r' + r" size="sm" mono>{{
            r
          }}</Badge>
          <Badge
            v-for="r in (row as Credential).scope!.streams"
            :key="'s' + r"
            size="sm"
            mono
            icon="branch"
            >{{ r }}</Badge
          >
          <Badge
            v-for="r in (row as Credential).scope!.operations"
            :key="'o' + r"
            size="sm"
            mono
            tone="accent"
            >{{ r }}</Badge
          >
        </span>
      </template>
      <template #cell-expires_at_ms="{ row }"
        ><span :title="formatDateTime((row as Credential).expires_at_ms)"
          ><RelativeTime :ms="(row as Credential).expires_at_ms" /></span
      ></template>
      <template #cell-last_used_ms="{ row }"
        ><span class="faint"
          ><RelativeTime
            v-if="(row as Credential).last_used_ms"
            :ms="(row as Credential).last_used_ms"
          /><template v-else>never</template></span
        ></template
      >
      <template #cell-actions="{ row }">
        <Button
          v-if="stateOf(row as Credential) === 'active'"
          size="sm"
          variant="ghost"
          icon="trash"
          label="Revoke"
          @click="revoking = row as Credential"
        />
      </template>
      <template #empty
        ><EmptyState
          icon="key"
          compact
          title="No credentials"
          description="Mint a token for yourself, or (as an administrator) for an agent or service."
      /></template>
    </DataTable>

    <Dialog
      v-model="minting"
      :title="minted ? 'Your new token' : 'Mint a token'"
      size="lg"
      :description="
        minted
          ? undefined
          : 'A token never has more power than you do, and never outlives the credential that minted it.'
      "
      :dismissable="!busy"
    >
      <div v-if="minted" class="reveal" data-testid="minted">
        <Alert tone="warn" live="status" title="This is the only time you will see this token">
          Copy it now and store it somewhere safe. The server keeps only a hash, so it cannot show
          it again. If you lose it, revoke it and mint another.
        </Alert>
        <div class="tokenbox">
          <input
            class="token mono"
            :class="{ masked: !revealed }"
            type="text"
            autocomplete="off"
            spellcheck="false"
            readonly
            :value="minted.token"
            aria-label="New token"
            data-testid="minted-token"
            @focus="($event.target as HTMLInputElement).select()"
          />
          <Button
            size="sm"
            :icon="revealed ? 'eye-off' : 'eye'"
            :label="revealed ? 'Hide token' : 'Show token'"
            @click="revealed = !revealed"
          />
          <CopyButton :text="minted.token" label="Copy token" with-label size="md" />
        </div>
        <dl class="facts">
          <div>
            <dt>For</dt>
            <dd><IdentityLabel :identity="minted.credential.subject" /></dd>
          </div>
          <div>
            <dt>Expires</dt>
            <dd>{{ formatDateTime(minted.credential.expires_at_ms) }}</dd>
          </div>
          <div>
            <dt>Credential id</dt>
            <dd class="mono">{{ minted.credential.id }}</dd>
          </div>
        </dl>
      </div>
      <form v-else id="mint-form" class="form" @submit.prevent="mint">
        <TextField
          v-model="form.subject"
          label="Identity"
          mono
          :placeholder="auth.identity"
          :hint="
            isAdmin
              ? 'Leave empty to mint for yourself. Administrators can mint for agent:… or service:… identities.'
              : 'Leave empty: you can only mint tokens for yourself.'
          "
          :disabled="!isAdmin"
        />
        <TextField
          v-model="form.description"
          label="Description"
          placeholder="What is this token for?"
          autofocus
        />
        <SelectField
          v-model="form.ttl"
          label="Lifetime"
          :options="TTLS"
          hint="The server caps lifetimes (agents: at most an hour by default)."
        />
        <fieldset class="scope">
          <legend>Limit what it can do (optional)</legend>
          <TextArea
            v-model="form.repos"
            label="Repositories"
            :rows="1"
            mono
            placeholder="acme/webapp, acme/*"
            hint="Empty means every repository your roles allow."
          />
          <TextArea
            v-model="form.streams"
            label="Streams"
            :rows="1"
            mono
            placeholder="main, agents/*"
          />
          <TextArea
            v-model="form.operations"
            label="Operations"
            :rows="1"
            mono
            placeholder="proposal.create, objects.write"
          />
        </fieldset>
        <Alert v-if="error" tone="bad" live="alert" title="The server refused">{{
          error.message
        }}</Alert>
      </form>
      <template #footer>
        <template v-if="minted"
          ><Button variant="primary" data-testid="minted-done" @click="minting = false"
            >I have copied it</Button
          ></template
        >
        <template v-else>
          <Button :disabled="busy" @click="minting = false">Cancel</Button>
          <Button type="submit" form="mint-form" variant="primary" :loading="busy" icon="key"
            >Mint token</Button
          >
        </template>
      </template>
    </Dialog>

    <Dialog
      v-model="revokeOpen"
      title="Revoke this credential?"
      alert
      size="sm"
      :dismissable="!revokeBusy"
      description="Anything using it stops working immediately. This cannot be undone."
    >
      <p v-if="revoking" class="faint mono">{{ revoking.id }} · {{ revoking.subject }}</p>
      <template #footer>
        <Button :disabled="revokeBusy" @click="revoking = null">Keep it</Button>
        <Button variant="danger" :loading="revokeBusy" @click="revoke">Revoke</Button>
      </template>
    </Dialog>
  </Card>
</template>

<style scoped>
.tools {
  display: flex;
  align-items: flex-end;
  gap: var(--s-4);
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}
.subj {
  flex: 1;
  min-width: 260px;
  max-width: 460px;
}
.id {
  font-size: var(--fs-xs);
  display: flex;
  gap: 6px;
  align-items: center;
}
.chips {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.form,
.reveal {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.scope {
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: var(--s-3);
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
legend {
  font-weight: 580;
  font-size: var(--fs-sm);
  padding: 0 4px;
}
.tokenbox {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}
.token {
  flex: 1;
  min-width: 240px;
  padding: 8px 10px;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  background: var(--code-bg);
  color: var(--text);
}
.masked {
  -webkit-text-security: disc;
  text-security: disc;
}
.facts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: var(--s-2) var(--s-4);
  margin: 0;
  font-size: var(--fs-sm);
}
dt {
  color: var(--text-3);
  font-size: var(--fs-xs);
}
dd {
  margin: 0;
  overflow-wrap: anywhere;
}
</style>
