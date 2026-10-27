<script setup lang="ts">
import { useToasts } from '@/stores/toasts'
import type { IconName } from './icons'
import Icon from './Icon.vue'
import Button from './Button.vue'

const toasts = useToasts()
const ICON: Record<string, IconName> = { info: 'info', ok: 'check', warn: 'alert', bad: 'alert' }
</script>

<template>
  <Teleport to="body">
    <!-- A polite live region: new toasts are announced without stealing focus. Errors are assertive. -->
    <div class="host" role="region" aria-label="Notifications">
      <TransitionGroup name="toast" tag="ul" class="list" role="list">
        <li
          v-for="t in toasts.items"
          :key="t.id"
          class="toast"
          :class="`t-${t.tone}`"
          @mouseenter="toasts.pause(t.id)"
          @mouseleave="toasts.resume(t.id)"
          @focusin="toasts.pause(t.id)"
          @focusout="toasts.resume(t.id)"
        >
          <Icon :name="ICON[t.tone] ?? 'info'" :size="18" class="lead" />
          <div class="text" :role="t.tone === 'bad' ? 'alert' : 'status'">
            <p class="title">{{ t.title }}</p>
            <p v-if="t.message" class="msg">{{ t.message }}</p>
            <p v-if="t.requestId" class="rid mono">request {{ t.requestId }}</p>
          </div>
          <Button
            variant="ghost"
            size="sm"
            icon="x"
            label="Dismiss notification"
            @click="toasts.dismiss(t.id)"
          />
        </li>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.host {
  position: fixed;
  right: var(--s-4);
  bottom: var(--s-4);
  z-index: var(--z-toast);
  width: min(380px, calc(100vw - 2 * var(--s-4)));
  pointer-events: none;
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.toast {
  pointer-events: auto;
  display: flex;
  gap: var(--s-3);
  align-items: flex-start;
  padding: var(--s-3);
  border-radius: var(--r-md);
  background: var(--surface);
  border: 1px solid var(--border);
  border-left-width: 4px;
  box-shadow: var(--shadow-2);
}
.t-ok {
  border-left-color: var(--ok);
}
.t-ok .lead {
  color: var(--ok);
}
.t-warn {
  border-left-color: var(--warn);
}
.t-warn .lead {
  color: var(--warn);
}
.t-bad {
  border-left-color: var(--bad);
}
.t-bad .lead {
  color: var(--bad);
}
.t-info {
  border-left-color: var(--info);
}
.t-info .lead {
  color: var(--info);
}
.lead {
  margin-top: 2px;
}
.text {
  flex: 1;
  min-width: 0;
}
.title {
  font-weight: 620;
}
.msg {
  color: var(--text-2);
  font-size: var(--fs-sm);
  margin-top: 2px;
  overflow-wrap: anywhere;
}
.rid {
  color: var(--text-3);
  font-size: var(--fs-xs);
  margin-top: 4px;
  overflow-wrap: anywhere;
}
.toast-enter-active,
.toast-leave-active {
  transition:
    opacity var(--dur-2) var(--ease),
    transform var(--dur-2) var(--ease);
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateY(8px);
}
</style>
