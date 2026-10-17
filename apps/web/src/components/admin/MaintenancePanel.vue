<script setup lang="ts">
import { ref } from 'vue'
import { api } from '@/api/instance'
import type { FsckResponse, GcResponse } from '@/api/types'
import { formatBytes, formatNumber } from '@/lib/format'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import Dialog from '@/ui/Dialog.vue'
import KeyValue from '@/ui/KeyValue.vue'
import type { KvItem } from '@/ui/types'

const props = defineProps<{ repo: string }>()
const toasts = useToasts()

const dry = ref<GcResponse | null>(null)
const real = ref<GcResponse | null>(null)
const fsck = ref<FsckResponse | null>(null)
const busy = ref<'' | 'dry' | 'real' | 'fsck'>('')
const confirming = ref(false)

async function dryRun() {
  busy.value = 'dry'
  real.value = null
  try {
    dry.value = await api.admin.gc(props.repo, { dry_run: true })
  } catch (e) {
    toasts.error('Dry run failed', e)
  } finally {
    busy.value = ''
  }
}
async function runForReal() {
  busy.value = 'real'
  try {
    real.value = await api.admin.gc(props.repo, { dry_run: false })
    confirming.value = false
    dry.value = null
    toasts.ok(
      'Garbage collection finished',
      `${formatNumber(real.value.deleted)} objects removed, ${formatBytes(real.value.bytes)} freed`,
    )
  } catch (e) {
    toasts.error('Garbage collection failed', e)
  } finally {
    busy.value = ''
  }
}
async function runFsck() {
  busy.value = 'fsck'
  try {
    fsck.value = await api.admin.fsck(props.repo)
  } catch (e) {
    toasts.error('Integrity check failed', e)
  } finally {
    busy.value = ''
  }
}

const gcItems = (g: GcResponse): KvItem[] => [
  { key: 'rc', label: 'Reachable changesets', value: formatNumber(g.reachable_changesets) },
  { key: 'rb', label: 'Reachable blobs', value: formatNumber(g.reachable_blobs) },
  { key: 'sc', label: 'Objects scanned', value: formatNumber(g.scanned) },
  {
    key: 'del',
    label: g.dry_run ? 'Would delete' : 'Deleted',
    value: `${formatNumber(g.deleted)} objects (${formatBytes(g.bytes)})`,
  },
  { key: 'ky', label: 'Kept: too young', value: formatNumber(g.kept_young) },
  { key: 'ks', label: 'Kept: staged', value: formatNumber(g.kept_staged) },
]
</script>

<template>
  <div class="maint">
    <Card
      title="Garbage collection"
      icon="trash"
      subtitle="Removes objects nothing references. Always look at a dry run first."
    >
      <div class="stack">
        <p class="muted">
          GC fails closed and never deletes anything a writer just referenced: young objects are
          kept for a grace period, and pinned revisions are roots.
        </p>
        <div class="row">
          <Button icon="search" :loading="busy === 'dry'" data-testid="gc-dry" @click="dryRun"
            >Dry run</Button
          >
          <Button
            variant="danger"
            icon="trash"
            :disabled="!dry || dry.deleted === 0"
            :disabled-reason="!dry ? 'Run a dry run first' : 'Nothing to delete'"
            data-testid="gc-real"
            @click="confirming = true"
            >Collect for real…</Button
          >
        </div>
        <Alert v-if="dry" tone="info" title="Dry run: nothing was deleted" data-testid="gc-result">
          <KeyValue :items="gcItems(dry)" label-width="12rem" />
        </Alert>
        <Alert v-if="real" tone="ok" title="Collection finished">
          <KeyValue :items="gcItems(real)" label-width="12rem" />
        </Alert>
      </div>
    </Card>

    <Card
      title="Integrity check"
      icon="shield"
      subtitle="Re-reads and re-hashes every stored object."
    >
      <div class="stack">
        <div>
          <Button icon="shield" :loading="busy === 'fsck'" data-testid="fsck-run" @click="runFsck"
            >Run fsck</Button
          >
        </div>
        <Alert v-if="fsck && fsck.ok" tone="ok" title="Healthy" data-testid="fsck-result"
          >{{ formatNumber(fsck.objects_checked) }} objects checked, no problems found.</Alert
        >
        <Alert
          v-else-if="fsck"
          tone="bad"
          :title="`${fsck.issues.length} problem${fsck.issues.length === 1 ? '' : 's'} found`"
          data-testid="fsck-result"
        >
          <ul class="issues" role="list">
            <li v-for="(i, n) in fsck.issues" :key="n">
              <strong>{{ i.kind }}</strong> <code class="mono">{{ i.object }}</code>
              {{ i.detail }}
            </li>
          </ul>
        </Alert>
      </div>
    </Card>

    <Dialog
      v-model="confirming"
      alert
      size="sm"
      title="Delete unreferenced objects?"
      :dismissable="busy !== 'real'"
      :description="
        dry
          ? `${formatNumber(dry.deleted)} objects (${formatBytes(dry.bytes)}) will be removed. This cannot be undone.`
          : ''
      "
    >
      <template #footer>
        <Button :disabled="busy === 'real'" @click="confirming = false">Cancel</Button>
        <Button variant="danger" :loading="busy === 'real'" @click="runForReal">Delete them</Button>
      </template>
    </Dialog>
  </div>
</template>

<style scoped>
.maint {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.stack {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.row {
  display: flex;
  gap: var(--s-2);
  flex-wrap: wrap;
}
.issues {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
</style>
