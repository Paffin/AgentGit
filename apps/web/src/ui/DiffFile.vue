<script setup lang="ts">
import { computed, ref, useId, watch } from 'vue'
import { useScrollRegion } from '@/composables/useScrollRegion'
import { lineCount, parseUnifiedDiff } from '@/lib/unidiff'
import type { FileDiff } from '@/api/types'
import type { DiffLine } from '@/lib/diffDiscussion'
import Badge from './Badge.vue'
import Button from './Button.vue'
import Icon from './Icon.vue'

const props = withDefaults(
  defineProps<{
    file: FileDiff
    /** Start collapsed. Huge diffs always start collapsed. */
    collapsed?: boolean
    anchor?: string
    discussLines?: boolean
  }>(),
  { collapsed: false, anchor: undefined, discussLines: false },
)

const emit = defineEmits<{ discuss: [line: DiffLine] }>()
const LARGE = 600
const parsed = computed(() => parseUnifiedDiff(props.file.patch))
// Both gutters share the widest rendered number, including a digit boundary inside a hunk.
const numberWidth = computed(() => {
  let largest = 0
  for (const hunk of parsed.value.hunks) {
    for (const line of hunk.lines) largest = Math.max(largest, line.oldNo ?? 0, line.newNo ?? 0)
  }
  return `${Math.max(3, String(largest).length)}ch`
})
const total = computed(() => lineCount(parsed.value))
const forced = computed(() => total.value > LARGE)
const open = ref(!props.collapsed && !forced.value)
// Reset before rendering a replacement patch; identical refreshes keep the reader's choice.
watch([() => props.file.path, () => props.file.patch], () => {
  open.value = !props.collapsed && !forced.value
})
const bodyId = useId()
// A long line scrolls the diff sideways, inside its own box; the keyboard can reach it then.
const { setEl: diffRef, attrs: diffAttrs } = useScrollRegion(() => `Changes to ${props.file.path}`)

const modeNames: Record<string, string> = {
  '100644': 'Regular file',
  '100755': 'Executable file',
  '120000': 'Symbolic link',
}
const modeLabel = (mode?: string) => modeNames[mode || ''] || mode || 'Absent'
const statusTone = computed(() =>
  props.file.status === 'added' ? 'ok' : props.file.status === 'deleted' ? 'bad' : 'neutral',
)
const sigil = { add: '+', del: '−', ctx: ' ', meta: '\\' } as const
</script>

<template>
  <section :id="anchor" class="file" :aria-label="`Changes to ${file.path}`">
    <header class="head">
      <button
        type="button"
        class="toggle"
        :aria-expanded="open"
        :aria-controls="bodyId"
        :title="open ? 'Collapse this file' : 'Expand this file'"
        @click="open = !open"
      >
        <Icon :name="open ? 'chevron-down' : 'chevron-right'" :size="16" />
        <span class="path mono">{{ file.path }}</span>
      </button>
      <slot name="actions" />
      <span class="meta">
        <Badge size="sm" :tone="statusTone">{{ file.status }}</Badge>
        <span v-if="file.binary" class="faint">binary</span>
        <template v-else>
          <span class="plus mono">+{{ file.additions }}</span>
          <span class="minus mono">−{{ file.deletions }}</span>
        </template>
      </span>
    </header>
    <div
      v-if="open"
      :id="bodyId"
      :ref="diffRef"
      class="diff"
      :style="{ '--number-width': numberWidth }"
      v-bind="diffAttrs"
    >
      <p v-if="file.old_mode !== file.new_mode && (file.old_mode || file.new_mode)" class="note">
        {{ modeLabel(file.old_mode) }} → {{ modeLabel(file.new_mode) }}
        <span class="mono"
          >({{ file.old_mode || 'absent' }} → {{ file.new_mode || 'absent' }})</span
        >
      </p>
      <p v-if="file.truncated" class="note">
        Text diff omitted because it exceeds the size limit. Inspect the full file before reviewing.
      </p>
      <p v-else-if="file.binary" class="note">Binary file: its contents are not shown.</p>
      <p v-else-if="parsed.hunks.length === 0" class="note">No textual changes.</p>
      <template v-else>
        <div v-for="(h, hi) in parsed.hunks" :key="hi" class="hunk">
          <div class="hunk-head mono">
            <span class="hh">{{ h.header.slice(0, h.header.indexOf('@@', 2) + 2) }}</span>
            <span v-if="h.section" class="sec">{{ h.section }}</span>
          </div>
          <div v-for="(l, li) in h.lines" :key="li" class="row mono" :class="`k-${l.kind}`">
            <template v-for="side in ['old', 'new'] as const" :key="side">
              <button
                v-if="discussLines && (side === 'old' ? l.oldNo : l.newNo) != null"
                type="button"
                class="no line-action"
                :aria-label="`Discuss ${side} line ${side === 'old' ? l.oldNo : l.newNo} of ${file.path}`"
                title="Quote this line in the discussion"
                @click="
                  emit('discuss', {
                    path: file.path,
                    side,
                    line: (side === 'old' ? l.oldNo : l.newNo)!,
                    text: l.text,
                  })
                "
              >
                {{ side === 'old' ? l.oldNo : l.newNo }}
              </button>
              <span v-else class="no" aria-hidden="true">{{
                side === 'old' ? l.oldNo : l.newNo
              }}</span>
            </template>
            <span class="sg" aria-hidden="true">{{ sigil[l.kind] }}</span>
            <span v-if="l.kind === 'add'" class="sr-only">added: </span>
            <span v-else-if="l.kind === 'del'" class="sr-only">removed: </span>
            <span class="txt">{{ l.text }}</span>
          </div>
        </div>
      </template>
    </div>
    <div v-else-if="forced" class="collapsed-note">
      <span class="faint">{{ total.toLocaleString('en-US') }} lines hidden.</span>
      <Button size="sm" @click="open = true">Show diff</Button>
    </div>
  </section>
</template>

<style scoped>
.file {
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  background: var(--surface);
  overflow: hidden;
}
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  padding: 0 var(--s-3);
  background: var(--surface-2);
  border-bottom: 1px solid var(--border);
  min-height: 38px;
}
.toggle {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  background: none;
  border: none;
  padding: var(--s-2) 0;
  min-width: 0;
  text-align: left;
  color: var(--text);
}
.path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 560;
}
.meta {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  flex: none;
  font-size: var(--fs-sm);
}
.plus {
  color: var(--ok);
}
.minus {
  color: var(--bad);
}
.diff {
  overflow-x: auto;
  font-size: var(--fs-sm);
  line-height: 20px;
}
.note,
.collapsed-note {
  padding: var(--s-3) var(--s-4);
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.collapsed-note {
  display: flex;
  align-items: center;
  gap: var(--s-3);
}
.hunk {
  min-width: max-content;
}
.hunk-head {
  display: flex;
  gap: var(--s-3);
  padding: 2px var(--s-3);
  background: var(--diff-hunk-bg);
  color: var(--text-2);
  font-size: var(--fs-xs);
}
.sec {
  color: var(--text-3);
}
.row {
  display: flex;
  white-space: pre;
}
.no {
  flex: none;
  width: calc(var(--number-width) + 12px);
  padding: 0 6px;
  text-align: right;
  color: var(--text-3);
  user-select: none;
  background: var(--surface-2);
}
.line-action {
  border: 0;
  border-radius: 0;
  font: inherit;
  cursor: pointer;
}
.line-action:hover,
.line-action:focus-visible {
  text-decoration: underline;
  color: var(--accent);
  outline-offset: -2px;
}
.sg {
  flex: none;
  width: 2.2ch;
  text-align: center;
  user-select: none;
}
.txt {
  flex: 1;
  padding-right: var(--s-4);
}
.k-add {
  background: var(--diff-add-bg);
}
.k-add .no {
  background: var(--diff-add-gutter);
  color: var(--diff-add-text);
}
.k-add .line-action {
  border: 0;
  border-radius: 0;
  font: inherit;
  cursor: pointer;
}
.line-action:hover,
.line-action:focus-visible {
  text-decoration: underline;
  color: var(--accent);
  outline-offset: -2px;
}
.sg {
  color: var(--diff-add-text);
}
.k-del {
  background: var(--diff-del-bg);
}
.k-del .no {
  background: var(--diff-del-gutter);
  color: var(--diff-del-text);
}
.k-del .line-action {
  border: 0;
  border-radius: 0;
  font: inherit;
  cursor: pointer;
}
.line-action:hover,
.line-action:focus-visible {
  text-decoration: underline;
  color: var(--accent);
  outline-offset: -2px;
}
.sg {
  color: var(--diff-del-text);
}
.k-meta {
  color: var(--text-3);
  font-style: italic;
}
</style>
