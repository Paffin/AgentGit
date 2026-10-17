<script setup lang="ts">
import { computed } from 'vue'
import type { FeedStatus } from '@/api/sse'

const props = withDefaults(defineProps<{ status: FeedStatus; retryInMs?: number }>(), {
  retryInMs: 0,
})

const view = computed(() => {
  switch (props.status) {
    case 'live':
      return { tone: 'live', text: 'Live', detail: 'Receiving updates in real time' }
    case 'connecting':
      return { tone: 'wait', text: 'Connecting', detail: 'Opening the live event stream' }
    case 'reconnecting':
      return {
        tone: 'warn',
        text: 'Reconnecting',
        detail:
          props.retryInMs > 0
            ? `Lost the event stream; retrying in ${Math.ceil(props.retryInMs / 1000)}s`
            : 'Lost the event stream',
      }
    case 'unauthenticated':
      return {
        tone: 'bad',
        text: 'Signed out',
        detail: 'The credential expired or was revoked',
      }
    case 'denied':
      return {
        tone: 'bad',
        text: 'No access',
        detail: 'This credential may not follow events here',
      }
    default:
      return { tone: 'idle', text: 'Paused', detail: 'Not receiving updates' }
  }
})
</script>

<template>
  <span
    class="live"
    :class="`t-${view.tone}`"
    role="status"
    :title="view.detail"
    data-testid="live-indicator"
    :data-status="status"
  >
    <span class="dot" aria-hidden="true" />
    <span class="text">{{ view.text }}</span>
    <span class="sr-only">. {{ view.detail }}</span>
  </span>
</template>

<style scoped>
.live {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 26px;
  padding: 0 10px;
  border-radius: var(--r-pill);
  border: 1px solid var(--l-line);
  background: var(--l-bg);
  color: var(--l-fg);
  font-size: var(--fs-xs);
  font-weight: 600;
  white-space: nowrap;
}
.dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: currentColor;
}
.t-live {
  --l-fg: var(--ok);
  --l-bg: var(--ok-soft);
  --l-line: var(--ok-line);
}
.t-live .dot {
  animation: pulse 2.4s ease-out infinite;
}
.t-wait,
.t-idle {
  --l-fg: var(--text-2);
  --l-bg: var(--surface-2);
  --l-line: var(--border);
}
.t-warn {
  --l-fg: var(--warn);
  --l-bg: var(--warn-soft);
  --l-line: var(--warn-line);
}
.t-warn .dot {
  border-radius: 2px;
}
.t-bad {
  --l-fg: var(--bad);
  --l-bg: var(--bad-soft);
  --l-line: var(--bad-line);
}
@keyframes pulse {
  0% {
    box-shadow: 0 0 0 0 color-mix(in srgb, currentColor 55%, transparent);
  }
  70%,
  100% {
    box-shadow: 0 0 0 7px transparent;
  }
}
@media (prefers-reduced-motion: reduce) {
  .t-live .dot {
    animation: none;
  }
}
</style>
