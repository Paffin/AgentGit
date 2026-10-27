<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api } from '@/api/instance'
import type { Changeset, DiffResponse } from '@/api/types'
import { useResource } from '@/composables/useResource'
import { formatDateTime, plural, shortId } from '@/lib/format'
import Badge from '@/ui/Badge.vue'
import Card from '@/ui/Card.vue'
import CopyButton from '@/ui/CopyButton.vue'
import DiffViewer from '@/ui/DiffViewer.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{ repo: string; id: string }>()
const route = useRoute()

const cs = useResource<Changeset>(
  (signal) => api.history.changeset(props.repo, props.id, true, { signal }),
  {
    watch: [() => props.repo, () => props.id],
  },
)
// Diff against the first parent: exactly what this changeset's delta did. A root changeset diffs against nothing.
const diff = useResource<DiffResponse>(
  async (signal) => {
    const c = cs.data.value
    if (!c) return { files: [], truncated: false }
    return api.tree.diff(props.repo, { from: c.parents[0] ?? '', to: c.id, context: 3 }, { signal })
  },
  { immediate: false },
)
const loadedDiff = computed(() => diff.data.value)
async function showDiff() {
  await diff.reload()
}
</script>

<template>
  <Card title="Changeset" icon="commit" data-testid="changeset-detail">
    <div v-if="!cs.settled.value" class="sk"><Skeleton :lines="4" /></div>
    <ErrorState
      v-else-if="cs.error.value"
      what="this changeset"
      :error="cs.error.value"
      compact
      @retry="cs.reload()"
    />
    <template v-else-if="cs.data.value">
      <div class="detail">
        <p class="msg">{{ cs.data.value.message || '(no message)' }}</p>
        <dl class="kv">
          <div>
            <dt>Id</dt>
            <dd class="mono">
              {{ shortId(cs.data.value.id, 20) }}
              <CopyButton :text="cs.data.value.id" label="Copy changeset id" />
            </dd>
          </div>
          <div>
            <dt>Author</dt>
            <dd><IdentityLabel :identity="cs.data.value.author" /></dd>
          </div>
          <div>
            <dt>When</dt>
            <dd>{{ formatDateTime(cs.data.value.timestamp_ms) }}</dd>
          </div>
          <div>
            <dt>
              {{ cs.data.value.parents.length > 1 ? 'Parents (merge)' : 'Parent' }}
            </dt>
            <dd>
              <span v-if="cs.data.value.parents.length === 0" class="faint">none (root)</span>
              <span v-for="p in cs.data.value.parents" :key="p" class="par">
                <RouterLink
                  :to="{ path: `/repos/${repo}/history`, query: { ...route.query, cs: p } }"
                  class="mono"
                  >{{ shortId(p, 10) }}</RouterLink
                >
              </span>
            </dd>
          </div>
          <div>
            <dt>Generation</dt>
            <dd>{{ cs.data.value.generation }}</dd>
          </div>
        </dl>
        <section aria-labelledby="ops-h">
          <h3 id="ops-h">{{ plural(cs.data.value.ops.length, 'file operation') }}</h3>
          <ul class="ops" role="list">
            <li v-for="o in cs.data.value.ops.slice(0, 200)" :key="o.path">
              <Badge size="sm" :tone="o.op === 'delete' ? 'bad' : 'ok'">{{
                o.op === 'delete' ? 'delete' : 'put'
              }}</Badge>
              <code class="mono">{{ o.path }}</code>
              <span v-if="o.executable" class="faint">executable</span>
              <span
                v-if="o.file_type && o.file_type !== 'file' && o.file_type !== 'regular'"
                class="faint"
                >{{ o.file_type }}</span
              >
            </li>
            <li v-if="cs.data.value.ops.length > 200" class="faint">
              and {{ cs.data.value.ops.length - 200 }} more
            </li>
          </ul>
        </section>
        <section aria-labelledby="diff-h">
          <h3 id="diff-h">Changes</h3>
          <p v-if="!loadedDiff && !diff.loading.value" class="faint">
            <button type="button" class="link" @click="showDiff">Show the diff</button>
            against the first parent.
          </p>
          <Skeleton v-if="diff.loading.value && !loadedDiff" :lines="4" />
          <ErrorState
            v-else-if="diff.error.value && !loadedDiff"
            what="the diff"
            :error="diff.error.value"
            compact
            @retry="diff.reload()"
          />
          <DiffViewer
            v-else-if="loadedDiff"
            :files="loadedDiff.files"
            :truncated="loadedDiff.truncated"
            anchor-prefix="cs-diff"
          />
        </section>
      </div>
    </template>
  </Card>
</template>

<style scoped>
.sk {
  padding: var(--s-2);
}
.detail {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.msg {
  font-size: var(--fs-lg);
  font-weight: 600;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.kv {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-2) var(--s-4);
  margin: 0;
}
dt {
  font-size: var(--fs-xs);
  color: var(--text-3);
}
dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.par {
  margin-right: var(--s-3);
}
h3 {
  font-size: var(--fs-md);
  margin-bottom: var(--s-2);
}
.ops {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: var(--fs-sm);
}
.ops li {
  display: flex;
  gap: var(--s-2);
  align-items: center;
  flex-wrap: wrap;
}
.link {
  background: none;
  border: none;
  padding: 0;
  color: var(--accent-text);
  text-decoration: underline;
  text-underline-offset: 0.18em;
}
</style>
