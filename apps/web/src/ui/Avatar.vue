<script setup lang="ts">
import { computed } from 'vue'
import { parseIdentity } from '@/lib/identity'
import { ICONS } from './icons'

/**
 * One avatar for every kind of actor, told apart by shape and glyph (not only by colour):
 * people are circles with initials, agents are hexagons with a robot, services are rounded squares
 * with a server, the system is a diamond with a bolt.
 */
const props = withDefaults(
  defineProps<{ identity: string; size?: number; decorative?: boolean }>(),
  {
    size: 24,
    decorative: false,
  },
)

const parsed = computed(() => parseIdentity(props.identity))

function hash(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return h >>> 0
}

const HUES = {
  user: [214, 232, 160, 28, 340, 190],
  agent: [262, 284, 308, 246],
  service: [172, 188, 200],
  system: [220],
  unknown: [220],
} as const

const hue = computed(() => {
  const set = HUES[parsed.value.kind]
  return set[hash(parsed.value.principal) % set.length]
})

const initials = computed(() => {
  const words = parsed.value.name.split(/[^A-Za-z0-9]+/).filter(Boolean)
  const letters =
    words.length >= 2
      ? (words[0]?.charAt(0) ?? '') + (words[1]?.charAt(0) ?? '')
      : (words[0] ?? '?').slice(0, 2)
  return letters.toUpperCase()
})

const kindLabel = computed(() => parsed.value.kind)
</script>

<template>
  <svg
    class="avatar"
    :class="`k-${parsed.kind}`"
    :style="{ '--h': hue }"
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    :role="decorative ? undefined : 'img'"
    :aria-label="decorative ? undefined : `${kindLabel}: ${identity}`"
    :aria-hidden="decorative ? 'true' : undefined"
    focusable="false"
  >
    <title v-if="!decorative">{{ identity }}</title>
    <template v-if="parsed.kind === 'agent'">
      <path class="shape" d="M12 1.6l9.4 5.4v10L12 22.4 2.6 17V7z" />
      <g transform="translate(6.2 6.4) scale(.52)" class="glyph">
        <path :d="ICONS.bot" />
      </g>
    </template>
    <template v-else-if="parsed.kind === 'service'">
      <rect class="shape" x="2" y="2" width="20" height="20" rx="6" />
      <g transform="translate(6.2 6.2) scale(.52)" class="glyph">
        <path :d="ICONS.server" />
      </g>
    </template>
    <template v-else-if="parsed.kind === 'system'">
      <path class="shape" d="M12 1.5l10.5 10.5L12 22.5 1.5 12z" />
      <g transform="translate(6.4 6.4) scale(.5)" class="glyph">
        <path :d="ICONS.bolt" />
      </g>
    </template>
    <template v-else>
      <circle class="shape" cx="12" cy="12" r="10.6" />
      <text class="initials" x="12" y="12" text-anchor="middle" dominant-baseline="central">
        {{ initials }}
      </text>
    </template>
  </svg>
</template>

<style scoped>
.avatar {
  flex: none;
  --av-bg: light-dark(hsl(var(--h) 70% 92%), hsl(var(--h) 35% 22%));
  --av-fg: light-dark(hsl(var(--h) 60% 27%), hsl(var(--h) 75% 85%));
  --av-line: light-dark(hsl(var(--h) 45% 72%), hsl(var(--h) 35% 40%));
  display: inline-block;
  vertical-align: middle;
}
.shape {
  fill: var(--av-bg);
  stroke: var(--av-line);
  stroke-width: 1.2;
  stroke-linejoin: round;
}
.glyph {
  fill: none;
  stroke: var(--av-fg);
  stroke-width: 2.1;
  stroke-linecap: round;
  stroke-linejoin: round;
}
.initials {
  fill: var(--av-fg);
  font-size: 9.2px;
  font-weight: 650;
  font-family: var(--font-sans);
  letter-spacing: 0.02em;
}
.k-agent .shape {
  stroke-dasharray: none;
}
</style>
