<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { usePageTitle } from '@/composables/usePageTitle'
import { safeNext } from '@/router'
import { useAuth } from '@/stores/auth'
import Button from '@/ui/Button.vue'
import EmptyState from '@/ui/EmptyState.vue'

const auth = useAuth()
const route = useRoute()

const ended = computed(() => auth.endedBecause === 'expired')
const next = computed(() =>
  typeof route.query.next === 'string' ? safeNext(route.query.next) : '/',
)
usePageTitle(() => (ended.value ? 'Session ended' : 'Not signed in'))
</script>

<template>
  <div class="wrap">
    <h1 class="sr-only">{{ ended ? 'Your session ended' : 'You are not signed in' }}</h1>
    <EmptyState
      icon="lock"
      :title="ended ? 'Your session ended' : 'You are not signed in'"
      :description="
        ended
          ? 'The server stopped accepting your credential. It expired or was revoked, or the sign-in it came from ended. Nothing you did here was lost; sign in again to carry on.'
          : 'This page needs a signed-in session. Sign in to continue.'
      "
    >
      <p v-if="ended && auth.rejection" class="detail" data-testid="unauthorized-detail">
        The server said: {{ auth.rejection.message }}
        <span v-if="auth.rejection.requestId" class="faint mono"
          >(request {{ auth.rejection.requestId }})</span
        >
      </p>
      <p v-if="next !== '/'" class="faint">
        You come back to <code>{{ next }}</code> afterwards.
      </p>
      <Button
        :to="{ name: 'signin', query: next === '/' ? {} : { next } }"
        variant="primary"
        icon="key"
        data-testid="sign-in-again"
        >Sign in again</Button
      >
    </EmptyState>
  </div>
</template>

<style scoped>
.wrap {
  margin-top: var(--s-7);
}
.detail {
  margin: 0;
  color: var(--text-2);
}
</style>
