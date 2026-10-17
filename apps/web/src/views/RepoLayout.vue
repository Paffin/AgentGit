<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink, RouterView, useRoute } from 'vue-router'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import Badge from '@/ui/Badge.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Icon from '@/ui/Icon.vue'
import type { IconName } from '@/ui/icons'

const props = defineProps<{ owner: string; name: string }>()
const board = useBoard()
const auth = useAuth()
const route = useRoute()

const full = computed(() => `${props.owner}/${props.name}`)

watch(
  full,
  (repo) => {
    void board.start(repo)
  },
  { immediate: true },
)
onBeforeUnmount(() => board.stop())

interface Tab {
  to: string
  label: string
  icon: IconName
  exact?: boolean
}
const tabs = computed<Tab[]>(() => {
  const base = `/repos/${encodeURIComponent(props.owner)}/${encodeURIComponent(props.name)}`
  const list: Tab[] = [
    { to: base, label: 'Live board', icon: 'activity', exact: true },
    { to: `${base}/proposals`, label: 'Proposals', icon: 'proposal' },
    { to: `${base}/streams`, label: 'Streams', icon: 'branch' },
    { to: `${base}/history`, label: 'History', icon: 'history' },
    { to: `${base}/files`, label: 'Files', icon: 'folder' },
    { to: `${base}/compare`, label: 'Compare', icon: 'diff' },
    { to: `${base}/agents`, label: 'Agents', icon: 'bot' },
  ]
  if (auth.hasRole(full.value, 'admin'))
    list.push({ to: `${base}/admin`, label: 'Admin', icon: 'shield' })
  return list
})

// On a narrow screen the tab strip scrolls sideways: keep the current tab in view instead of off to one side.
const strip = ref<HTMLElement | null>(null)
function revealCurrent() {
  strip.value
    ?.querySelector<HTMLElement>('.tab.current')
    ?.scrollIntoView({ inline: 'center', block: 'nearest' })
}
onMounted(revealCurrent)
watch(
  () => route.path,
  () => void nextTick(revealCurrent),
)

const repoError = computed(() =>
  board.load.repo.status === 'error' ? board.load.repo.error : null,
)
</script>

<template>
  <div class="repo">
    <div class="strip">
      <div class="crumb">
        <RouterLink to="/" class="back" aria-label="All repositories"
          ><Icon name="chevron-left" :size="16"
        /></RouterLink>
        <span class="path mono" data-testid="repo-name"
          >{{ owner }}<span class="slash">/</span><strong>{{ name }}</strong></span
        >
        <Badge v-if="board.repo" mono icon="branch" size="sm">{{
          board.repo.default_stream
        }}</Badge>
      </div>
      <p v-if="board.repo?.description" class="desc">{{ board.repo.description }}</p>
    </div>

    <nav ref="strip" class="tabs" aria-label="Repository">
      <RouterLink
        v-for="t in tabs"
        :key="t.to"
        :to="t.to"
        class="tab"
        :exact-active-class="t.exact ? 'current' : undefined"
        :active-class="t.exact ? undefined : 'current'"
      >
        <Icon :name="t.icon" :size="15" />
        {{ t.label }}
      </RouterLink>
    </nav>

    <ErrorState
      v-if="repoError"
      what="this repository"
      :error="repoError"
      @retry="board.refreshAll()"
    >
      <RouterLink to="/" class="back-link">Back to repositories</RouterLink>
    </ErrorState>
    <RouterView v-else :key="full" />
  </div>
</template>

<style scoped>
.repo {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.strip {
  display: flex;
  align-items: center;
  gap: var(--s-4);
  flex-wrap: wrap;
  min-height: 28px;
}
.crumb {
  display: flex;
  align-items: center;
  gap: var(--s-2);
}
.back {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  border-radius: var(--r-sm);
  color: var(--text-2);
}
.back:hover {
  background: var(--surface-3);
  text-decoration: none;
}
.path {
  font-size: var(--fs-lg);
  color: var(--text-2);
}
.path strong {
  color: var(--text);
}
.slash {
  margin: 0 2px;
  color: var(--text-3);
}
.desc {
  color: var(--text-2);
  font-size: var(--fs-sm);
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}
.tabs {
  display: flex;
  gap: var(--s-1);
  border-bottom: 1px solid var(--border);
  overflow-x: auto;
  scrollbar-width: none;
}
.tab {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 0 var(--s-3);
  height: 40px;
  min-height: var(--touch-min);
  flex: none;
  color: var(--text-2);
  font-weight: 560;
  border-bottom: 2px solid transparent;
  margin-bottom: -1px;
  white-space: nowrap;
}
.tab:hover {
  color: var(--text);
  text-decoration: none;
  background: var(--surface-2);
}
.tab.current {
  color: var(--text);
  border-bottom-color: var(--accent);
}
.back-link {
  font-weight: 560;
}
</style>
