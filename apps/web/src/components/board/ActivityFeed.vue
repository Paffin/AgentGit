<script setup lang="ts">
import { computed } from 'vue'
import ActivityRow from '@/components/ActivityRow.vue'
import { useNow } from '@/composables/useNow'
import { plural } from '@/lib/format'
import { useLive } from '@/stores/live'
import { usePrefs } from '@/stores/prefs'
import Card from '@/ui/Card.vue'
import Checkbox from '@/ui/Checkbox.vue'
import EmptyState from '@/ui/EmptyState.vue'
import Skeleton from '@/ui/Skeleton.vue'
import VirtualList from '@/ui/VirtualList.vue'
import type { EventView } from '@/lib/events'

defineProps<{ repo: string; height?: string }>()
const live = useLive()
const prefs = usePrefs()
const now = useNow()

const items = computed(() =>
  prefs.showMinorEvents ? live.events : live.events.filter((e) => !e.minor),
)
const hiddenCount = computed(() => live.events.length - items.value.length)
const isFresh = (e: EventView) => e.liveAt > 0 && now.value - e.liveAt < 3000
</script>

<template>
  <Card
    title="Activity"
    icon="activity"
    :subtitle="`${plural(items.length, 'event')}, newest first`"
    flush
  >
    <template #actions>
      <Checkbox
        v-model="prefs.showMinorEvents"
        label="Housekeeping"
        :hint="hiddenCount ? `${hiddenCount} hidden` : undefined"
      />
    </template>
    <div class="body" data-testid="activity-feed">
      <div v-if="live.status === 'connecting' && items.length === 0" class="sk">
        <Skeleton :lines="4" />
      </div>
      <EmptyState
        v-else-if="items.length === 0"
        icon="activity"
        compact
        title="No activity yet"
        description="Every change in this repository shows up here the moment it happens."
      />
      <VirtualList
        v-else
        :items="items"
        :row-height="52"
        :item-key="(e: EventView) => e.seq"
        :height="height ?? '460px'"
        label="Repository activity"
        stick-to-top
      >
        <template #default="{ item }">
          <ActivityRow :event="item" :repo="repo" :fresh="isFresh(item)" />
        </template>
      </VirtualList>
    </div>
  </Card>
</template>

<style scoped>
.body {
  min-height: 120px;
}
.sk {
  padding: var(--s-4);
}
</style>
