<script setup lang="ts">
import { useId } from 'vue'
import Icon from './Icon.vue'
import Term from './Term.vue'
import type { TermId } from '@/lib/glossary'
import type { IconName } from './icons'

withDefaults(
  defineProps<{
    title?: string
    subtitle?: string
    icon?: IconName
    /** Remove body padding (tables, lists that bring their own). */
    flush?: boolean
    /** Heading level for the title; the page decides, the card only styles. */
    level?: 2 | 3 | 4
    tone?: 'default' | 'warn' | 'bad' | 'ok'
    /** Show the title through the glossary: its plain label, with the AgentGit word in a tooltip. */
    term?: TermId
  }>(),
  {
    title: undefined,
    subtitle: undefined,
    icon: undefined,
    flush: false,
    level: 2,
    tone: 'default',
    term: undefined,
  },
)

const titleId = useId()
</script>

<template>
  <section class="card" :class="[`tone-${tone}`]" :aria-labelledby="title ? titleId : undefined">
    <header v-if="title || $slots.actions || $slots.header" class="head">
      <slot name="header">
        <div class="titles">
          <component :is="`h${level}`" :id="titleId" class="title">
            <Icon v-if="icon" :name="icon" :size="16" />
            <Term v-if="term" :id="term" plural />
            <template v-else>{{ title }}</template>
          </component>
          <p v-if="subtitle" class="subtitle">{{ subtitle }}</p>
        </div>
      </slot>
      <div v-if="$slots.actions" class="actions"><slot name="actions" /></div>
    </header>
    <div class="body" :class="{ flush }"><slot /></div>
    <footer v-if="$slots.footer" class="foot"><slot name="footer" /></footer>
  </section>
</template>

<style scoped>
.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  box-shadow: none;
  min-width: 0;
}
.tone-warn {
  border-color: var(--warn-line);
}
.tone-bad {
  border-color: var(--bad-line);
}
.tone-ok {
  border-color: var(--ok-line);
}
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  padding: var(--s-3) var(--s-4);
  border-bottom: 1px solid var(--border);
  min-height: 48px;
}
.titles {
  min-width: 0;
}
.title {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  font-size: var(--fs-md);
  font-weight: 600;
  letter-spacing: 0;
}
.title :deep(.icon) {
  color: var(--text-3);
}
.subtitle {
  color: var(--text-3);
  font-size: var(--fs-xs);
  margin-top: 2px;
}
.actions {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex: none;
}
.body {
  padding: var(--s-4);
}
.body.flush {
  padding: 0;
}
.foot {
  padding: var(--s-3) var(--s-4);
  border-top: 1px solid var(--border);
  color: var(--text-2);
  font-size: var(--fs-sm);
}
</style>
