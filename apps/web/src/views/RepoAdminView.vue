<script setup lang="ts">
import { computed, ref } from 'vue'
import AuditPanel from '@/components/admin/AuditPanel.vue'
import CredentialsManager from '@/components/admin/CredentialsManager.vue'
import MaintenancePanel from '@/components/admin/MaintenancePanel.vue'
import PinsPanel from '@/components/admin/PinsPanel.vue'
import PolicyEditor from '@/components/admin/PolicyEditor.vue'
import RolesManager from '@/components/admin/RolesManager.vue'
import SettingsPanel from '@/components/admin/SettingsPanel.vue'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import EmptyState from '@/ui/EmptyState.vue'
import Tabs from '@/ui/Tabs.vue'
import type { TabItem } from '@/ui/types'

const { repo } = useRepoName()
const auth = useAuth()
usePageTitle(() => `Admin · ${repo.value}`)

type TabId = 'policy' | 'roles' | 'credentials' | 'settings' | 'maintenance' | 'audit' | 'pins'
const tab = ref<TabId>('policy')
const tabs: TabItem<TabId>[] = [
  { id: 'policy', label: 'Policy', icon: 'shield' },
  { id: 'roles', label: 'Roles', icon: 'user' },
  { id: 'credentials', label: 'Credentials', icon: 'key' },
  { id: 'settings', label: 'Settings and quota', icon: 'sliders' },
  { id: 'maintenance', label: 'GC and fsck', icon: 'trash' },
  { id: 'audit', label: 'Audit log', icon: 'list' },
  { id: 'pins', label: 'Pins', icon: 'pin' },
]
const isAdmin = computed(() => auth.hasRole(repo.value, 'admin'))
const defaultRepos = computed(() => [repo.value])
</script>

<template>
  <div class="page">
    <div>
      <h1>Administration</h1>
      <p class="muted">
        Policy, access and maintenance for <code class="mono">{{ repo }}</code
        >. Every change here is written to the audit log.
      </p>
    </div>
    <EmptyState
      v-if="!isAdmin"
      icon="lock"
      title="Administration needs the admin role"
      description="You can read this repository, but changing its policy, roles or settings requires the admin role on it."
    />
    <template v-else>
      <Alert tone="info"
        >The server decides what each action may do. If you see “permission denied” here, your
        credential’s scope is narrower than your role.</Alert
      >
      <Tabs v-model="tab" :tabs="tabs" label="Administration">
        <template #policy><PolicyEditor :repo="repo" /></template>
        <template #roles><RolesManager :repo="repo" /></template>
        <template #credentials
          ><CredentialsManager title="Credentials" :default-repos="defaultRepos"
        /></template>
        <template #settings><SettingsPanel :repo="repo" /></template>
        <template #maintenance><MaintenancePanel :repo="repo" /></template>
        <template #audit><AuditPanel :repo="repo" /></template>
        <template #pins><PinsPanel :repo="repo" /></template>
      </Tabs>
    </template>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
</style>
