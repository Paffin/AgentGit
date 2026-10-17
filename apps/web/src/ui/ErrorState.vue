<script setup lang="ts">
import { computed } from 'vue'
import { ApiError } from '@/api/errors'
import { describeError } from '@/api/errors'
import Button from './Button.vue'
import Icon from './Icon.vue'
import type { IconName } from './icons'

const props = withDefaults(
  defineProps<{
    error: unknown
    /** Show a retry button and emit `retry` when pressed. */
    retryable?: boolean
    compact?: boolean
    /** What was being loaded, for the headline: "Could not load proposals". */
    what?: string
  }>(),
  { retryable: true, compact: false, what: 'this' },
)

defineEmits<{ retry: [] }>()

const api = computed(() => (props.error instanceof ApiError ? props.error : null))

const view = computed<{ icon: IconName; title: string; body: string }>(() => {
  const e = api.value
  if (e?.isNetwork) {
    return {
      icon: 'wifi-off',
      title: 'Cannot reach the server',
      body: 'Check that the server is running and your network is up. The page keeps trying in the background.',
    }
  }
  if (e?.isForbidden) {
    return {
      icon: 'lock',
      title: 'You do not have access',
      body: e.message || 'Your role or credential scope does not allow this.',
    }
  }
  if (e?.isNotFound) {
    return {
      icon: 'search',
      title: `Could not find ${props.what}`,
      body: 'It does not exist, or you are not allowed to know that it does.',
    }
  }
  if (e?.isUnauthenticated) {
    return {
      icon: 'key',
      title: 'Please sign in again',
      body: 'Your credential is missing, invalid or expired.',
    }
  }
  if (e?.code === 'unimplemented') {
    return { icon: 'alert', title: 'Not available on this server', body: e.message }
  }
  return {
    icon: 'alert',
    title: `Could not load ${props.what}`,
    body: describeError(props.error),
  }
})
</script>

<template>
  <div class="err" :class="{ compact }" role="alert">
    <span class="glyph"><Icon :name="view.icon" :size="compact ? 18 : 22" /></span>
    <p class="title">{{ view.title }}</p>
    <p class="body">{{ view.body }}</p>
    <p v-if="api?.requestId" class="rid mono">request {{ api.requestId }}</p>
    <div class="actions">
      <slot />
      <Button
        v-if="retryable && !api?.isForbidden && !api?.isUnauthenticated"
        icon="refresh"
        @click="$emit('retry')"
      >
        Try again
      </Button>
    </div>
  </div>
</template>

<style scoped>
.err {
  display: flex;
  flex-direction: column;
  align-items: center;
  text-align: center;
  gap: var(--s-2);
  padding: var(--s-6) var(--s-4);
}
.compact {
  padding: var(--s-4) var(--s-3);
}
.glyph {
  display: grid;
  place-items: center;
  width: 44px;
  height: 44px;
  border-radius: 50%;
  background: var(--bad-soft);
  color: var(--bad);
  border: 1px solid var(--bad-line);
}
.compact .glyph {
  width: 34px;
  height: 34px;
}
.title {
  font-weight: 640;
}
.body {
  color: var(--text-2);
  max-width: 52ch;
  font-size: var(--fs-sm);
  overflow-wrap: anywhere;
}
.rid {
  color: var(--text-3);
  font-size: var(--fs-xs);
}
.actions {
  display: flex;
  gap: var(--s-2);
  margin-top: var(--s-2);
}
.actions:empty {
  display: none;
}
</style>
