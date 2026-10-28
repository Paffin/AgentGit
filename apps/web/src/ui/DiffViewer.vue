<script setup lang="ts">
import { computed } from 'vue'
import type { FileDiff } from '@/api/types'
import type { DiffLine } from '@/lib/diffDiscussion'
import { plural } from '@/lib/format'
import DiffFile from './DiffFile.vue'
import EmptyState from './EmptyState.vue'
import Icon from './Icon.vue'

const props = withDefaults(
  defineProps<{
    files: readonly FileDiff[]
    /** The server stopped early: more files changed than were returned. */
    truncated?: boolean
    /** Prefix for per-file anchors (`#<prefix>-3`). */
    emptyTitle?: string
    emptyDescription?: string
    anchorPrefix?: string
    discussLines?: boolean
  }>(),
  {
    truncated: false,
    discussLines: false,
    anchorPrefix: 'diff',
    emptyTitle: 'No differences',
    emptyDescription: 'The two revisions have the same files.',
  },
)

defineEmits<{ discuss: [line: DiffLine] }>()
const totals = computed(() => ({
  additions: props.files.reduce((n, f) => n + f.additions, 0),
  deletions: props.files.reduce((n, f) => n + f.deletions, 0),
}))
</script>

<template>
  <div class="viewer">
    <div v-if="truncated" class="trunc" role="note">
      <Icon name="alert" :size="16" />
      The diff was cut off by the server because it is too large. Some changed files are missing
      from this view.
    </div>
    <EmptyState
      v-if="files.length === 0"
      icon="diff"
      :title="emptyTitle"
      :description="emptyDescription"
      compact
    />
    <template v-else>
      <p class="summary">
        {{ plural(files.length, 'file') }} changed,
        <span class="plus">+{{ totals.additions }}</span>
        <span class="minus">−{{ totals.deletions }}</span>
      </p>
      <div class="files">
        <DiffFile
          v-for="(f, i) in files"
          :key="f.path"
          :file="f"
          :anchor="`${anchorPrefix}-${i}`"
          :discuss-lines="discussLines"
          @discuss="$emit('discuss', $event)"
        >
          <template #actions><slot name="file-actions" :file="f" /></template>
        </DiffFile>
      </div>
    </template>
  </div>
</template>

<style scoped>
.summary {
  color: var(--text-2);
  font-size: var(--fs-sm);
  margin-bottom: var(--s-3);
}
.plus {
  color: var(--ok);
  font-weight: 600;
  margin-left: 4px;
}
.minus {
  color: var(--bad);
  font-weight: 600;
  margin-left: 4px;
}
.trunc {
  display: flex;
  gap: var(--s-2);
  align-items: flex-start;
  padding: var(--s-3);
  border: 1px solid var(--warn-line);
  background: var(--warn-soft);
  color: var(--warn);
  border-radius: var(--r-sm);
  margin-bottom: var(--s-3);
  font-size: var(--fs-sm);
}
.files {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
</style>
