<script setup lang="ts">
import { ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { safeNext } from '@/router/redirect'
import { usePageTitle } from '@/composables/usePageTitle'
import Button from '@/ui/Button.vue'
import EmptyState from '@/ui/EmptyState.vue'

const route = useRoute()
const router = useRouter()
const busy = ref(false)
usePageTitle(() => 'Offline')

async function retry() {
  busy.value = true
  try {
    const next = typeof route.query.next === 'string' ? safeNext(route.query.next) : '/'
    await router.replace(next)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <div class="wrap">
    <h1 class="sr-only">Offline</h1>
    <EmptyState
      icon="wifi-off"
      title="Cannot reach the server"
      description="Your saved session is kept. Check that the AgentGit server is running and that your network is up, then try again."
    >
      <Button variant="primary" icon="refresh" :loading="busy" @click="retry">Try again</Button>
    </EmptyState>
  </div>
</template>

<style scoped>
.wrap {
  margin-top: var(--s-7);
}
</style>
