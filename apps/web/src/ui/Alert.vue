<script setup lang="ts">
import { computed } from 'vue'
import Icon from './Icon.vue'
import type { IconName } from './icons'

const props = withDefaults(
  defineProps<{
    tone?: 'info' | 'ok' | 'warn' | 'bad'
    title?: string
    /** `alert` interrupts assistive tech; use it for errors the user just caused. */
    live?: 'off' | 'status' | 'alert'
  }>(),
  { tone: 'info', title: undefined, live: 'off' },
)

const ICON: Record<string, IconName> = { info: 'info', ok: 'check', warn: 'alert', bad: 'alert' }
const role = computed(() => (props.live === 'off' ? undefined : props.live))
</script>

<template>
  <div class="alert" :class="`t-${tone}`" :role="role">
    <Icon :name="ICON[tone] ?? 'info'" :size="16" class="lead" />
    <div class="text">
      <p v-if="title" class="title">{{ title }}</p>
      <div class="body"><slot /></div>
    </div>
  </div>
</template>

<style scoped>
.alert {
  display: flex;
  gap: var(--s-2);
  padding: var(--s-3);
  border-radius: var(--r-sm);
  border: 1px solid var(--a-line);
  background: var(--a-bg);
  color: var(--text);
  font-size: var(--fs-sm);
}
.lead {
  color: var(--a-fg);
  margin-top: 2px;
}
.text {
  min-width: 0;
  flex: 1;
}
.title {
  font-weight: 640;
  color: var(--a-fg);
}
.body {
  overflow-wrap: anywhere;
}
.body :deep(p + p) {
  margin-top: var(--s-1);
}
.t-info {
  --a-fg: var(--info);
  --a-bg: var(--info-soft);
  --a-line: var(--info-line);
}
.t-ok {
  --a-fg: var(--ok);
  --a-bg: var(--ok-soft);
  --a-line: var(--ok-line);
}
.t-warn {
  --a-fg: var(--warn);
  --a-bg: var(--warn-soft);
  --a-line: var(--warn-line);
}
.t-bad {
  --a-fg: var(--bad);
  --a-bg: var(--bad-soft);
  --a-line: var(--bad-line);
}
</style>
