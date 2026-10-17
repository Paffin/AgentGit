<script setup lang="ts">
import CredentialsManager from '@/components/admin/CredentialsManager.vue'
import RolesManager from '@/components/admin/RolesManager.vue'
import WhoamiCard from '@/components/WhoamiCard.vue'
import { usePageTitle } from '@/composables/usePageTitle'
import { useAuth } from '@/stores/auth'
import Card from '@/ui/Card.vue'

const auth = useAuth()
usePageTitle(() => 'Access')
</script>

<template>
  <div class="page">
    <div>
      <h1>Access</h1>
      <p class="muted">Who you are, what you may do, and the tokens that act for you.</p>
    </div>
    <Card v-if="auth.principal" title="You" icon="user">
      <WhoamiCard :principal="auth.principal" />
    </Card>
    <CredentialsManager />
    <RolesManager v-if="auth.isAdminAnywhere" />
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  max-width: 1100px;
}
</style>
