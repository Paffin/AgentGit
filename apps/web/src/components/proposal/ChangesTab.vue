<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { DiffResponse, FileDiff } from '@/api/types'
import type { DiffLine, LineQuote } from '@/lib/diffDiscussion'
import { shortId } from '@/lib/format'
import TextField from '@/ui/TextField.vue'
import Button from '@/ui/Button.vue'
import Badge from '@/ui/Badge.vue'
import DiffViewer from '@/ui/DiffViewer.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{
  diff?: DiffResponse & { base: string; head?: string }
  reviewScope?: string
  canDiscuss?: boolean
  loading: boolean
  error: unknown
}>()
const emit = defineEmits<{ retry: []; discuss: [quote: LineQuote] }>()
function discuss(line: DiffLine) {
  if (props.canDiscuss && props.diff?.head)
    emit('discuss', { ...line, base: props.diff.base, head: props.diff.head })
}

const query = ref('')
const hideViewed = ref(false)
const viewed = ref<string[]>([])
const storageError = ref('')
const storageKey = computed(() =>
  props.reviewScope && props.diff?.head
    ? 'agentgit.review.v1:' + JSON.stringify([props.reviewScope, props.diff.base, props.diff.head])
    : '',
)
watch(
  storageKey,
  (key) => {
    viewed.value = []
    hideViewed.value = false
    storageError.value = ''
    if (!key) return
    try {
      const saved: unknown = JSON.parse(sessionStorage.getItem(key) || '[]')
      if (!Array.isArray(saved) || !saved.every((path) => typeof path === 'string'))
        throw new Error('Invalid progress')
      viewed.value = saved
    } catch {
      storageError.value = 'Review progress could not be restored. You can still review every file.'
    }
  },
  { immediate: true, flush: 'sync' },
)
const visibleFiles = computed(() =>
  (props.diff?.files || []).filter(
    (file) =>
      file.path.toLocaleLowerCase().includes(query.value.trim().toLocaleLowerCase()) &&
      (!hideViewed.value || !viewed.value.includes(file.path)),
  ),
)
const fileAnchors = computed(
  () => new Map((props.diff?.files || []).map((file, index) => [file.path, `diff-${index}`])),
)
const anchorForFile = (file: FileDiff) => fileAnchors.value.get(file.path) || ''
const viewedCount = computed(
  () => props.diff?.files.filter((file) => viewed.value.includes(file.path)).length || 0,
)
function mark(path: string, checked: boolean) {
  viewed.value = checked
    ? [...new Set([...viewed.value, path])]
    : viewed.value.filter((value) => value !== path)
  if (!storageKey.value) return
  try {
    sessionStorage.setItem(storageKey.value, JSON.stringify(viewed.value))
    storageError.value = ''
  } catch {
    storageError.value = 'Review progress is only in memory because this tab could not save it.'
  }
}
function showAll() {
  query.value = ''
  hideViewed.value = false
}
function jump(file: FileDiff) {
  const el = document.getElementById(anchorForFile(file))
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
      <div v-if="diff.files.length" class="review-tools">
        <TextField
          v-model="query"
          type="search"
          label="Filter changed files"
          placeholder="Filter changed files"
          hide-label
        />
        <label v-if="storageKey" class="hide-viewed"
          ><input v-model="hideViewed" type="checkbox" aria-label="Hide viewed files" /> Hide viewed
          files</label
        >
        <span v-if="storageKey" class="faint viewed-count" role="status"
          >{{ viewedCount }} of {{ diff.files.length }} loaded files viewed</span
        >
        <Button v-if="query || hideViewed" size="sm" @click="showAll">Show all files</Button>
      </div>
      <details class="comparison-details">
        <summary>Comparison details</summary>
        <p class="base faint">
          Comparing
          <code class="mono">{{ diff.base ? shortId(diff.base, 8) : 'the empty tree' }}</code>
          with the proposal’s head. Context is three lines.
        </p>
        <p v-if="storageKey" class="faint progress-note">
          Viewed marks are saved in this tab for this exact comparison. They do not approve the
          change request.
        </p>
      </details>
      <p v-if="storageError" role="status" class="faint">{{ storageError }}</p>
      <div class="layout" :class="{ 'with-nav': diff.files.length > 1 }">
        <nav v-if="diff.files.length > 1" class="files" aria-label="Changed files">
          <ul role="list">
            <li v-for="f in visibleFiles" :key="f.path">
              <a :href="`#${anchorForFile(f)}`" @click.prevent="jump(f)">
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
        <DiffViewer
          :files="visibleFiles"
          :anchor-for-file="anchorForFile"
          :discuss-lines="canDiscuss && !!diff.head"
          :truncated="diff.truncated"
          :empty-title="diff.files.length ? 'No matching files' : 'No differences'"
          :empty-description="
            diff.files.length
              ? 'Change the path filter or show viewed files to continue reviewing.'
              : 'The two revisions have the same files.'
          "
          @discuss="discuss"
        >
          <template v-if="storageKey" #file-actions="{ file }">
            <label class="viewed"
              ><input
                type="checkbox"
                :aria-label="`Viewed ${file.path}`"
                :checked="viewed.includes(file.path)"
                @change="mark(file.path, ($event.target as HTMLInputElement).checked)"
              />
              Viewed</label
            >
          </template>
        </DiffViewer>
      </div>
    </template>
  </div>
</template>

<style scoped>
/* Hallmark · shape: app screen · theme: locked AgentGit · scope: component · v2.0.0 */
/* Critique: filtering stays at hand; comparison guidance is disclosed and mobile file navigation stays one row. */
.review-tools {
  display: flex;
  align-items: end;
  gap: var(--s-3);
  flex-wrap: wrap;
  margin-bottom: var(--s-3);
}
.review-tools :deep(.field) {
  flex: 1;
  min-width: 200px;
}
.hide-viewed,
.viewed {
  display: flex;
  align-items: center;
  gap: var(--s-2);
  min-height: 38px;
  cursor: pointer;
  white-space: nowrap;
}
.viewed {
  font-size: var(--fs-xs);
}
.progress-note {
  margin-bottom: 0;
  font-size: var(--fs-xs);
}
input[type='checkbox'] {
  accent-color: var(--accent);
}

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
  gap: var(--s-1);
  padding: var(--s-2);
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
  gap: var(--s-1);
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
    grid-template-columns: 200px minmax(0, 1fr);
  }
  .files {
    order: 0;
  }
}

.comparison-details {
  margin-bottom: var(--s-3);
  color: var(--text-2);
  font-size: var(--fs-xs);
}
.comparison-details summary {
  cursor: pointer;
}
.comparison-details .base {
  margin-top: var(--s-2);
  margin-bottom: var(--s-2);
}
.viewed-count {
  font-size: var(--fs-sm);
}
@media (max-width: 700px) {
  .review-tools {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: var(--s-2);
    margin-bottom: var(--s-2);
  }
  .review-tools :deep(.field) {
    grid-column: 1 / -1;
    min-width: 0;
  }
  .viewed-count {
    text-align: right;
    font-size: var(--fs-xs);
  }
  .hide-viewed {
    font-size: var(--fs-sm);
  }
  .files {
    position: static;
    max-height: none;
  }
  .files ul {
    display: flex;
    gap: var(--s-1);
    overflow-x: auto;
  }
  .files li {
    flex: none;
    max-width: 85%;
  }
  .files a {
    min-height: 44px;
    flex-direction: row;
    align-items: center;
    gap: var(--s-3);
  }
  .files .p {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .files .n {
    flex: none;
  }
}
</style>
