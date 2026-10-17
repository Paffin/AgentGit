<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { isApiError } from '@/api/errors'
import type { BlameResponse } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { groupBlame } from '@/lib/blame'
import { plural, shortId } from '@/lib/format'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{ repo: string; rev: string; path: string }>()

/** A long file is shown in pieces: thousands of rows cost layout time nobody needs. */
const PAGE = 1000
const shown = ref(PAGE)
watch(
  () => [props.repo, props.rev, props.path],
  () => {
    shown.value = PAGE
  },
)

const blame = useResource<BlameResponse>(
  () => api.history.blame(props.repo, { path: props.path, rev: props.rev || undefined }),
  { watch: [() => props.repo, () => props.rev, () => props.path] },
)

// A server without the operation answers 405/501; say that, rather than showing a generic failure.
const unsupported = computed(() => {
  const e = blame.error.value
  return isApiError(e) && (e.status === 405 || e.status === 501)
})

const lines = computed(() => blame.data.value?.lines ?? [])
const visible = computed(() => lines.value.slice(0, shown.value))
const runs = computed(() => groupBlame(visible.value))
const boundaryCount = computed(() => lines.value.filter((l) => l.boundary).length)
</script>

<template>
  <div class="blame" data-testid="blame">
    <Skeleton v-if="!blame.settled.value" :lines="6" />
    <Alert v-else-if="unsupported" tone="info" title="Blame is not available on this server">
      This server does not offer line-by-line authorship. The file’s history is still available.
    </Alert>
    <ErrorState
      v-else-if="blame.error.value && !blame.data.value"
      what="the blame"
      :error="blame.error.value"
      compact
      @retry="blame.reload()"
    />
    <template v-else-if="blame.data.value">
      <p class="note faint">
        Each line shows the changeset that last changed it, found by walking history ({{
          plural(blame.data.value.visited, 'changeset')
        }}
        visited). Renames are followed.
      </p>
      <Alert v-if="blame.data.value.truncated" tone="warn" title="The walk stopped early">
        The server visited its maximum number of changesets before every line found its origin.
        Lines marked “older than shown” may really come from further back.
      </Alert>
      <p v-else-if="boundaryCount" class="note faint">
        {{ plural(boundaryCount, 'line') }} marked “older than shown”: the walk ended before their
        origin.
      </p>
      <p v-if="lines.length === 0" class="faint">This file is empty.</p>
      <div
        v-else
        class="scroll"
        tabindex="0"
        role="group"
        :aria-label="`Blame of ${path}, scrollable`"
      >
        <table :aria-label="`Line-by-line authorship of ${path}`">
          <thead class="sr-only">
            <tr>
              <th scope="col">Last changed by</th>
              <th scope="col">Line</th>
              <th scope="col">Text</th>
            </tr>
          </thead>
          <tbody>
            <template v-for="(run, ri) in runs" :key="`${ri}-${run.changeset}`">
              <tr
                v-for="(l, i) in run.lines"
                :key="l.line"
                :class="{ first: i === 0, boundary: run.boundary }"
                :data-changeset="run.changeset"
              >
                <td v-if="i === 0" class="who" :rowspan="run.lines.length">
                  <p class="line1">
                    <IdentityLabel :identity="run.author" :size="16" hide-run />
                    <RelativeTime :ms="run.timestampMs" class="faint" />
                  </p>
                  <p class="summary" :title="run.summary">{{ run.summary }}</p>
                  <p class="meta faint">
                    <RouterLink
                      :to="{ path: `/repos/${repo}/history`, query: { cs: run.changeset } }"
                      class="mono"
                      >{{ shortId(run.changeset, 8) }}</RouterLink
                    >
                    <Badge v-if="run.boundary" size="sm" tone="warn">older than shown</Badge>
                    <span
                      v-if="run.path !== path"
                      class="mono renamed"
                      :title="`Named ${run.path} then`"
                      >as {{ run.path }}</span
                    >
                  </p>
                </td>
                <td class="n mono">{{ l.line }}</td>
                <td class="code mono">{{ l.text }}</td>
              </tr>
            </template>
          </tbody>
        </table>
      </div>
      <div v-if="lines.length > shown" class="more">
        <Button size="sm" @click="shown += PAGE"
          >Show {{ Math.min(PAGE, lines.length - shown).toLocaleString('en-US') }} more
          lines</Button
        >
        <span class="faint"
          >{{ shown.toLocaleString('en-US') }} of
          {{ lines.length.toLocaleString('en-US') }} shown</span
        >
      </div>
    </template>
  </div>
</template>

<style scoped>
.blame {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.note {
  font-size: var(--fs-sm);
}
.scroll {
  overflow: auto;
  max-height: 70vh;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--surface);
}
table {
  border-collapse: collapse;
  width: 100%;
  font-size: var(--fs-sm);
}
tr.first > td {
  border-top: 1px solid var(--border);
}
tr.first:first-child > td {
  border-top: 0;
}
td {
  vertical-align: top;
  padding: 0 var(--s-2);
}
.who {
  width: 17rem;
  min-width: 14rem;
  max-width: 17rem;
  padding: var(--s-2) var(--s-3);
  border-right: 1px solid var(--border);
  background: var(--surface-2);
}
tr.boundary .who {
  background: var(--warn-soft);
}
.line1 {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}
.summary {
  margin: 2px 0;
  font-weight: 560;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.meta {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
  font-size: var(--fs-xs);
}
.renamed {
  overflow-wrap: anywhere;
}
.n {
  text-align: right;
  color: var(--text-3);
  width: 3.5rem;
  user-select: none;
  font-size: var(--fs-xs);
  line-height: 1.55;
}
.code {
  white-space: pre;
  line-height: 1.55;
  font-size: var(--fs-xs);
}
.more {
  display: flex;
  gap: var(--s-3);
  align-items: center;
  font-size: var(--fs-sm);
}
</style>
