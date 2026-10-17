<script setup lang="ts">
import { computed, ref } from 'vue'
import AgentsList from '@/components/agents/AgentsList.vue'
import LeaseTable from '@/components/agents/LeaseTable.vue'
import RunsTable from '@/components/agents/RunsTable.vue'
import TaskBoard from '@/components/agents/TaskBoard.vue'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { useBoard } from '@/stores/board'
import Card from '@/ui/Card.vue'
import Tabs from '@/ui/Tabs.vue'
import type { TabItem } from '@/ui/types'

const { repo } = useRepoName()
const board = useBoard()
usePageTitle(() => `Agents · ${repo.value}`)

type TabId = 'runs' | 'tasks' | 'leases' | 'agents'
const tab = ref<TabId>('runs')
const tabs = computed<TabItem<TabId>[]>(() => [
  { id: 'runs', label: 'Runs', icon: 'play', count: board.activeRuns.length || undefined },
  { id: 'tasks', label: 'Tasks', icon: 'task' },
  { id: 'leases', label: 'Leases', icon: 'lock', count: board.activeLeases.length || undefined },
  {
    id: 'agents',
    label: 'Registered agents',
    icon: 'bot',
    count: board.agents.length || undefined,
  },
])
</script>

<template>
  <div class="page">
    <div>
      <h1>Agents</h1>
      <p class="muted">
        Who is registered, what they are running, what they own and what they have claimed.
      </p>
    </div>
    <Card>
      <Tabs v-model="tab" :tabs="tabs" label="Agent coordination">
        <template #runs><RunsTable :repo="repo" /></template>
        <template #tasks><TaskBoard :repo="repo" /></template>
        <template #leases><LeaseTable :repo="repo" /></template>
        <template #agents><AgentsList :repo="repo" /></template>
      </Tabs>
    </Card>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
</style>
