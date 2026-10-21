<script setup lang="ts">
import { usePageTitle } from '@/composables/usePageTitle'
import { useAuth } from '@/stores/auth'
import Button from '@/ui/Button.vue'
import EmptyState from '@/ui/EmptyState.vue'

const auth = useAuth()
usePageTitle(() => 'No access')
</script>

<template>
  <div class="wrap">
    <h1 class="sr-only">No access</h1>
    <EmptyState
      icon="lock"
      title="You do not have access to this"
      :description="`${auth.identity || 'This credential'} has no role that allows it. The server also answers “not found” for repositories you may not know exist, so ask an administrator if you expected to see it.`"
    >
      <Button to="/" variant="primary" icon="home">Go to your home page</Button>
      <Button to="/access" icon="key">See my roles</Button>
    </EmptyState>
  </div>
</template>

<style scoped>
.wrap {
  margin-top: var(--s-7);
}
</style>
