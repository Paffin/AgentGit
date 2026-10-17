<script setup lang="ts">
import { computed } from 'vue'
import { parseIdentity } from '@/lib/identity'
import { shortId } from '@/lib/format'
import Avatar from './Avatar.vue'

const props = withDefaults(
  defineProps<{
    identity: string
    size?: number
    /** Show the kind prefix (`agent:`). Off by default: the avatar already tells. */
    showKind?: boolean
    /** Hide the run suffix of agent identities. */
    hideRun?: boolean
  }>(),
  { size: 20, showKind: false, hideRun: false },
)

const parsed = computed(() => parseIdentity(props.identity))
const name = computed(() => (props.showKind ? parsed.value.principal : parsed.value.name))
</script>

<template>
  <span class="ident" :title="identity">
    <Avatar :identity="identity" :size="size" decorative />
    <span class="sr-only">{{ parsed.kind }}</span>
    <span class="name">{{ name }}</span>
    <span v-if="parsed.run && !hideRun" class="run mono"
      >#{{ shortId(parsed.run.replace(/^run_/, ''), 6) }}</span
    >
  </span>
</template>

<style scoped>
.ident {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  max-width: 100%;
  vertical-align: middle;
}
.name {
  font-weight: 560;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}
.run {
  color: var(--text-3);
  font-size: var(--fs-xs);
  flex: none;
}
</style>
