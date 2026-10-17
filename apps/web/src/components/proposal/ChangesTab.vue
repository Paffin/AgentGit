<script setup lang="ts">
import type { DiffResponse } from '@/api/types'
import { shortId } from '@/lib/format'
import Badge from '@/ui/Badge.vue'
import DiffViewer from '@/ui/DiffViewer.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Skeleton from '@/ui/Skeleton.vue'

defineProps<{
  diff?: DiffResponse & { base: string }
  loading: boolean
  error: unknown
}>()
defineEmits<{ retry: [] }>()

function jump(i: number) {
  const el = document.getElementById(`diff-${i}`)
  if (!el) return
  el.scrollIntoView({ block: 'start', behavior: 'auto' })
  el.querySelector<HTMLElement>('button')?.focus({ preventScroll: true })
}
</script>

<template>
  <div class="changes" data-testid="changes-tab">
    <div v-if="loading && !diff" class="sk"><Skeleton :lines="5" /></div>
    <ErrorState
      v-else-if="error && !diff"
      what="the changes"
      :error="error"
      compact
      @retry="$emit('retry')"
    />
    <template v-else-if="diff">
      <p class="base faint">
        Comparing
        <code class="mono">{{ diff.base ? shortId(diff.base, 8) : 'the empty tree' }}</code>
        with the proposal’s head. Context is three lines.
      </p>
      <div class="layout" :class="{ 'with-nav': diff.files.length > 1 }">
        <nav v-if="diff.files.length > 1" class="files" aria-label="Changed files">
          <ul role="list">
            <li v-for="(f, i) in diff.files" :key="f.path">
              <a :href="`#diff-${i}`" @click.prevent="jump(i)">
                <span class="p mono">{{ f.path }}</span>
                <span class="n">
                  <Badge
                    v-if="f.status !== 'modified'"
                    size="sm"
                    :tone="f.status === 'added' ? 'ok' : 'bad'"
                    >{{ f.status === 'added' ? 'new' : 'removed' }}</Badge
                  >
                  <template v-if="!f.binary"
                    ><span class="plus mono">+{{ f.additions }}</span>
                    <span class="minus mono">−{{ f.deletions }}</span></template
                  >
                  <span v-else class="faint">binary</span>
                </span>
              </a>
            </li>
          </ul>
        </nav>
        <DiffViewer :files="diff.files" :truncated="diff.truncated" />
      </div>
    </template>
  </div>
</template>

<style scoped>
.sk {
  padding: var(--s-3);
}
.base {
  font-size: var(--fs-sm);
  margin-bottom: var(--s-3);
}
.layout {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: var(--s-4);
  align-items: start;
}
.files {
  position: sticky;
  top: calc(var(--header-h) + var(--s-3));
  max-height: calc(100vh - var(--header-h) - 2 * var(--s-4));
  overflow: auto;
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  background: var(--surface);
  order: -1;
}
.files ul {
  list-style: none;
  margin: 0;
  padding: var(--s-1);
}
.files a {
  text-decoration: none;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px var(--s-2);
  border-radius: var(--r-sm);
  color: var(--text);
  font-size: var(--fs-sm);
}
.files a:hover {
  background: var(--surface-2);
}
.p {
  overflow-wrap: anywhere;
  font-size: var(--fs-xs);
}
.n {
  display: flex;
  gap: 6px;
  align-items: center;
  font-size: var(--fs-xs);
}
.plus {
  color: var(--ok);
}
.minus {
  color: var(--bad);
}
@media (min-width: 1100px) {
  .layout.with-nav {
    grid-template-columns: 260px minmax(0, 1fr);
  }
  .files {
    order: 0;
  }
}
</style>
