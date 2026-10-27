<script setup lang="ts">
import { onBeforeUnmount, ref } from 'vue'
import Button from './Button.vue'

const props = withDefaults(
  defineProps<{
    text: string
    label?: string
    size?: 'sm' | 'md'
    /** Show the label next to the icon. */
    withLabel?: boolean
  }>(),
  { label: 'Copy', size: 'sm', withLabel: false },
)

const copied = ref(false)
const failed = ref(false)
let timer: ReturnType<typeof setTimeout> | undefined

async function copy() {
  clearTimeout(timer)
  copied.value = false
  failed.value = false
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(props.text)
    } else {
      legacyCopy(props.text)
    }
    copied.value = true
  } catch {
    try {
      legacyCopy(props.text)
      copied.value = true
    } catch {
      failed.value = true
    }
  }
  clearTimeout(timer)
  timer = setTimeout(() => {
    copied.value = false
    failed.value = false
  }, 1800)
}

/** Fallback for insecure contexts (plain http on a LAN address), where the async clipboard API is unavailable. */
function legacyCopy(text: string) {
  const ta = document.createElement('textarea')
  ta.value = text
  ta.setAttribute('readonly', '')
  ta.style.position = 'fixed'
  ta.style.opacity = '0'
  document.body.appendChild(ta)
  // Capture focus now: an async clipboard rejection may arrive after the user moved on.
  const previous = document.activeElement
  try {
    ta.select()
    if (!document.execCommand('copy')) throw new Error('copy failed')
  } finally {
    ta.remove()
    if (previous instanceof HTMLElement && previous.isConnected)
      previous.focus({ preventScroll: true })
  }
}

onBeforeUnmount(() => clearTimeout(timer))
</script>

<template>
  <span class="copy">
    <Button
      variant="ghost"
      :size="size"
      :icon="copied ? 'check' : 'copy'"
      :label="withLabel ? undefined : label"
      @click="copy"
    >
      <template v-if="withLabel">{{ copied ? 'Copied' : failed ? 'Copy failed' : label }}</template>
    </Button>
    <span class="sr-only" role="status">{{
      copied ? 'Copied to clipboard' : failed ? 'Copy failed' : ''
    }}</span>
  </span>
</template>

<style scoped>
.copy {
  display: inline-flex;
}
</style>
