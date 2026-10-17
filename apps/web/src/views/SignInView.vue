<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ApiError, describeError } from '@/api/errors'
import type { Principal } from '@/api/types'
import WhoamiCard from '@/components/WhoamiCard.vue'
import { safeNext } from '@/router'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import Icon from '@/ui/Icon.vue'
import TextField from '@/ui/TextField.vue'
import { usePageTitle } from '@/composables/usePageTitle'

const auth = useAuth()
const route = useRoute()
const router = useRouter()
usePageTitle(() => 'Sign in')

const token = ref('')
const reveal = ref(false)
const busy = ref(false)
const error = ref<ApiError | null>(null)
const who = ref<Principal | null>(null)

const next = computed(() =>
  typeof route.query.next === 'string' ? safeNext(route.query.next) : '/',
)
const reason = computed(() => {
  switch (auth.endedBecause) {
    case 'expired':
      return 'Your session ended: the credential expired or was revoked. Sign in again to continue.'
    case 'rejected':
      return 'The saved credential is no longer valid. Paste a new one.'
    default:
      return ''
  }
})
const methods = computed(() => auth.capabilities?.auth_methods ?? [])

onMounted(() => {
  void auth.loadCapabilities()
})

async function submit() {
  if (busy.value) return
  busy.value = true
  error.value = null
  try {
    who.value = await auth.signIn(token.value)
    token.value = '' // the secret is no longer needed on screen
    reveal.value = false
    await nextTick()
    document.getElementById('continue')?.focus()
  } catch (e) {
    error.value =
      e instanceof ApiError
        ? e
        : new ApiError({ status: 0, code: 'internal', message: describeError(e) })
  } finally {
    busy.value = false
  }
}

async function proceed() {
  await router.replace(next.value)
}
</script>

<template>
  <div class="page">
    <div class="intro">
      <h1>Sign in to AgentGit</h1>
      <p class="muted">
        Paste a token to see live agent work, proposals and policy. It stays in this tab’s session
        storage and is never put in a URL.
      </p>
    </div>

    <Card v-if="!who" title="Token" icon="key" :level="2">
      <form class="form" novalidate @submit.prevent="submit">
        <Alert v-if="reason && !error" tone="info" live="status">{{ reason }}</Alert>
        <TextField
          v-model="token"
          label="Access token"
          :type="reveal ? 'text' : 'password'"
          mono
          required
          autocomplete="off"
          placeholder="agt_… or an OIDC token"
          hint="A scoped token (agt_…), an OIDC ID/access token, or the bootstrap token."
        >
          <template #suffix>
            <Button
              variant="ghost"
              size="sm"
              :icon="reveal ? 'eye-off' : 'eye'"
              :label="reveal ? 'Hide token' : 'Show token'"
              @click="reveal = !reveal"
            />
          </template>
        </TextField>
        <Alert v-if="error" tone="bad" live="alert" title="Could not sign in">
          <p>
            {{
              error.isUnauthenticated
                ? 'The server did not accept that token. It may be mistyped, expired or revoked.'
                : describeError(error)
            }}
          </p>
          <p v-if="error.requestId" class="faint mono">request {{ error.requestId }}</p>
        </Alert>
        <Button type="submit" variant="primary" :loading="busy" icon="arrow-right" block
          >Sign in</Button
        >
      </form>
    </Card>

    <Card v-else title="You are signed in" icon="check" tone="ok">
      <div class="who">
        <WhoamiCard :principal="who" />
        <div class="actions">
          <Button id="continue" variant="primary" icon-right="arrow-right" @click="proceed"
            >Continue</Button
          >
        </div>
      </div>
    </Card>

    <Card title="Which token do I use?" icon="info" :level="2">
      <ul class="help" role="list">
        <li>
          <strong>Scoped token.</strong> Minted by an administrator (<code
            >ag-server admin mint</code
          >
          or <code>POST /v1/credentials</code>) or by you from the Access page after signing in
          once. Opaque, always expiring, revocable.
        </li>
        <li>
          <strong>Single sign-on (OIDC).</strong> If your organisation configured OIDC, paste your
          <em>ID token</em> or <em>access token</em> from your identity provider. The server checks
          issuer, audience, expiry and signature, and maps your groups to roles. Tokens from other
          providers are rejected.
        </li>
        <li>
          <strong>Bootstrap token.</strong> The administrator token set in the server’s environment.
          Use it only to create real credentials, then remove it.
        </li>
      </ul>
      <p v-if="methods.length" class="server faint">
        This server accepts:
        <Badge v-for="m in methods" :key="m" mono>{{ m }}</Badge>
        <span v-if="auth.capabilities">
          · protocol {{ auth.capabilities.protocol_version }} · server
          {{ auth.capabilities.server_version }}</span
        >
      </p>
      <p class="note faint">
        <Icon name="shield" :size="14" /> The token is sent only to this server, in an Authorization
        header. Closing the tab signs you out.
      </p>
    </Card>
  </div>
</template>

<style scoped>
.page {
  max-width: 640px;
  margin: var(--s-6) auto 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.intro {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.who {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.actions {
  display: flex;
  justify-content: flex-end;
}
.help {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  font-size: var(--fs-sm);
  color: var(--text-2);
}
.help strong {
  color: var(--text);
}
.server {
  margin-top: var(--s-3);
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
  font-size: var(--fs-sm);
}
.note {
  margin-top: var(--s-3);
  font-size: var(--fs-sm);
  display: flex;
  gap: var(--s-2);
  align-items: center;
}
</style>
