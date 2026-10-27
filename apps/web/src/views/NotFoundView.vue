<script setup lang="ts">
import { useRoute } from 'vue-router'
import { usePageTitle } from '@/composables/usePageTitle'
import Button from '@/ui/Button.vue'
import EmptyState from '@/ui/EmptyState.vue'

const route = useRoute()
usePageTitle(() => 'Page not found')
</script>

<template>
  <div class="wrap">
    <h1 class="sr-only">Page not found</h1>
    <EmptyState
      icon="search"
      title="This page does not exist"
      :description="`There is nothing at ${route.fullPath}. It may have moved, or the link is mistyped.`"
    >
      <Button to="/" variant="primary" icon="home">Go to your home page</Button>
    </EmptyState>
  </div>
</template>

<style scoped>
.wrap {
  margin-top: var(--s-7);
}
</style>
