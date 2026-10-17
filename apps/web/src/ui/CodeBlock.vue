<script setup lang="ts">
import { computed } from 'vue'
import { splitLines } from '@/lib/text'
import CopyButton from './CopyButton.vue'
import VirtualList from './VirtualList.vue'

const props = withDefaults(
  defineProps<{
    code: string
    label: string
    lineNumbers?: boolean
    maxHeight?: string
    copyable?: boolean
    /** Wrap long lines instead of scrolling sideways (small snippets only). */
    wrap?: boolean
  }>(),
  { lineNumbers: true, maxHeight: '480px', copyable: true, wrap: false },
)

const VIRTUALIZE_ABOVE = 1500
const ROW = 20

const lines = computed(() => splitLines(props.code))
const virtual = computed(() => lines.value.length > VIRTUALIZE_ABOVE)
const gutter = computed(() => `${String(lines.value.length).length + 1}ch`)
</script>

<template>
  <div class="code" :style="{ '--gutter': gutter }">
    <div v-if="copyable" class="tools"><CopyButton :text="code" label="Copy contents" /></div>
    <VirtualList
      v-if="virtual"
      :items="lines"
      :row-height="ROW"
      :item-key="(_l: string, i: number) => i"
      :height="maxHeight"
      :label="label"
    >
      <template #default="{ item, index }">
        <div class="vline">
          <span v-if="lineNumbers" class="ln" aria-hidden="true">{{ index + 1 }}</span>
          <span class="txt">{{ item }}</span>
        </div>
      </template>
    </VirtualList>
    <pre
      v-else
      class="pre"
      :class="{ wrap }"
      :style="{ maxHeight }"
      tabindex="0"
      :aria-label="label"
    ><code><span v-for="(l, i) in lines" :key="i" class="line" :class="{ numbered: lineNumbers }">{{ l }}</span></code></pre>
  </div>
</template>

<style scoped>
.code {
  position: relative;
  background: var(--code-bg);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  font-family: var(--font-mono);
  font-size: var(--fs-sm);
  line-height: 20px;
}
.tools {
  position: absolute;
  top: 4px;
  right: 6px;
  z-index: 1;
  opacity: 0.85;
}
.pre {
  margin: 0;
  padding: var(--s-2) 0;
  overflow: auto;
  counter-reset: line;
  tab-size: 4;
}
.pre code {
  font: inherit;
  display: block;
  min-width: max-content;
}
.pre.wrap code {
  min-width: 0;
}
.pre.wrap .line {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.line {
  display: block;
  white-space: pre;
  padding: 0 var(--s-4);
  min-height: 20px;
}
.line.numbered {
  counter-increment: line;
  padding-left: calc(var(--gutter) + var(--s-5));
  position: relative;
}
.line.numbered::before {
  content: counter(line);
  position: absolute;
  left: 0;
  width: var(--gutter);
  margin-left: var(--s-3);
  text-align: right;
  color: var(--text-3);
  user-select: none;
}
.vline {
  display: flex;
  height: 20px;
  white-space: pre;
}
.ln {
  flex: none;
  width: var(--gutter);
  margin: 0 var(--s-3);
  text-align: right;
  color: var(--text-3);
  user-select: none;
}
.txt {
  padding-right: var(--s-4);
}
</style>
