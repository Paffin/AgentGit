<script setup lang="ts">
import { computed } from 'vue'
import type { Principal } from '@/api/types'
import { useServerNow } from '@/composables/useNow'
import { formatCountdown, formatDateTime } from '@/lib/format'
import Badge from '@/ui/Badge.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import KeyValue from '@/ui/KeyValue.vue'
import type { KvItem } from '@/ui/types'

const props = defineProps<{ principal: Principal }>()
const now = useServerNow()

const METHOD: Record<string, string> = {
  token: 'Scoped token',
  oidc: 'OIDC single sign-on',
  bootstrap: 'Bootstrap token (setup only)',
}

const items = computed<KvItem[]>(() => [
  { key: 'identity', label: 'Identity' },
  { key: 'kind', label: 'Kind', value: props.principal.kind },
  {
    key: 'method',
    label: 'Signed in with',
    value: METHOD[props.principal.method] ?? props.principal.method,
  },
  { key: 'expires', label: 'Expires' },
  { key: 'roles', label: 'Roles' },
  { key: 'scope', label: 'Credential scope' },
])

const scope = computed(() => {
  const s = props.principal.scope
  const parts: { label: string; values: string[] }[] = []
  if (s?.repos.length) parts.push({ label: 'repositories', values: s.repos })
  if (s?.streams.length) parts.push({ label: 'streams', values: s.streams })
  if (s?.operations.length) parts.push({ label: 'operations', values: s.operations })
  return parts
})
</script>

<template>
  <KeyValue :items="items" label-width="9rem">
    <template #value-identity
      ><IdentityLabel :identity="principal.identity" :size="22" show-kind
    /></template>
    <template #value-expires>
      <template v-if="principal.expires_at_ms">
        {{ formatDateTime(principal.expires_at_ms) }}
        <span class="faint">({{ formatCountdown(principal.expires_at_ms, now) }})</span>
      </template>
      <template v-else>never (this credential does not expire)</template>
    </template>
    <template #value-roles>
      <span v-if="principal.roles.length === 0" class="faint"
        >none: you cannot see any repository until an administrator grants you a role</span
      >
      <span v-else class="chips">
        <Badge v-for="r in principal.roles" :key="r" mono tone="accent">{{ r }}</Badge>
      </span>
    </template>
    <template #value-scope>
      <span v-if="scope.length === 0" class="faint"
        >unrestricted by scope (your roles still apply)</span
      >
      <span v-else class="scope">
        <span v-for="p in scope" :key="p.label">
          <span class="faint">{{ p.label }}:</span>
          <Badge v-for="v in p.values" :key="v" mono>{{ v }}</Badge>
        </span>
      </span>
    </template>
  </KeyValue>
</template>

<style scoped>
.chips,
.scope {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
.scope > span {
  display: inline-flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
</style>
