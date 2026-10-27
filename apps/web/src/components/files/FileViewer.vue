<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import { useResource } from '@/composables/useResource'
import { useAuth } from '@/stores/auth'
import { readDraft } from '@/lib/drafts'
import { formatBytes } from '@/lib/format'
import { decodeUtf8, imageMime, looksBinary, toDataUrl, baseName } from '@/lib/text'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import BlameTable from './BlameTable.vue'
import FileEditor from './FileEditor.vue'
import FileActions from './FileActions.vue'
import CodeBlock from '@/ui/CodeBlock.vue'
import ErrorState from '@/ui/ErrorState.vue'
import Segmented from '@/ui/Segmented.vue'
import Skeleton from '@/ui/Skeleton.vue'

const props = defineProps<{
  repo: string
  rev: string
  path: string
  /** The person may edit here: they hold a writing role and `rev` is a branch (so a change request has a target). */
  canEdit?: boolean
  executable?: boolean
}>()
/** Editing the text in place; leaving the file ends it. */
const editing = ref(false)
const auth = useAuth()
const hasDraft = computed(() => {
  if (editing.value || !auth.identity) return false
  try {
    return (
      readDraft(sessionStorage, {
        identity: auth.identity,
        repo: props.repo,
        stream: props.rev,
        path: props.path,
        isNew: false,
      }) !== null
    )
  } catch {
    return false // Opening the editor explains an unreadable stored draft without discarding it.
  }
})

/**
 * Whatever the listing says about size, the guard works while downloading: a declared Content-Length over
 * the limit is refused up front, and an undeclared one is cut off as it streams.
 */
const DEFAULT_LIMIT = 1024 * 1024
const HARD_LIMIT = 8 * 1024 * 1024
const limit = ref(DEFAULT_LIMIT)
/** Plain contents, or who last changed each line. */
const view = ref<'contents' | 'blame'>('contents')
const VIEWS = [
  { value: 'contents', label: 'Contents' },
  { value: 'blame', label: 'Blame' },
] as const

watch(
  () => [props.repo, props.rev, props.path],
  () => {
    limit.value = DEFAULT_LIMIT
    view.value = 'contents'
    editing.value = false
  },
)

const file = useResource(
  async (signal) => {
    const { repo, rev, path } = props
    const revision = await api.history.resolve(repo, rev, { signal })
    const [contents, location] = await Promise.all([
      api.tree.file(repo, revision, path, limit.value, { signal }),
      api.tree.resolve(repo, revision, path, { signal }),
    ])
    return {
      ...contents,
      revision,
      entry: location.entry,
      executable: location.entry?.executable === true,
    }
  },
  {
    watch: [() => props.repo, () => props.rev, () => props.path, limit],
  },
)

const content = computed(() => {
  const f = file.data.value
  if (!f || f.truncated) return null
  const binary = looksBinary(f.bytes)
  return {
    binary,
    bytes: f.bytes,
    text: binary ? '' : decodeUtf8(f.bytes),
    size: f.size ?? f.bytes.length,
  }
})
const image = computed(() => {
  const c = content.value
  const mime = imageMime(props.path)
  return c && c.binary && mime && c.bytes.length <= DEFAULT_LIMIT ? toDataUrl(c.bytes, mime) : ''
})
const tooBig = computed(() => file.data.value?.truncated === true)
const canRaise = computed(() => limit.value < HARD_LIMIT)

let downloadUrl = ''
function download() {
  const c = content.value
  if (!c) return
  if (downloadUrl) URL.revokeObjectURL(downloadUrl)
  downloadUrl = URL.createObjectURL(
    new Blob([c.bytes as Uint8Array<ArrayBuffer>], { type: 'application/octet-stream' }),
  )
  const a = document.createElement('a')
  a.href = downloadUrl
  a.download = baseName(props.path)
  a.rel = 'noopener'
  document.body.appendChild(a)
  a.click()
  a.remove()
}
onBeforeUnmount(() => {
  if (downloadUrl) URL.revokeObjectURL(downloadUrl)
})
</script>

<template>
  <div class="viewer" data-testid="file-viewer">
    <Skeleton v-if="!file.settled.value" :lines="6" />
    <ErrorState
      v-else-if="file.error.value"
      what="this file"
      :error="file.error.value"
      compact
      @retry="file.reload()"
    />
    <template v-else-if="tooBig">
      <FileActions
        v-if="canEdit && file.data.value?.entry"
        :repo="repo"
        :rev="file.data.value.revision"
        :stream="rev"
        :entry="file.data.value.entry"
      />
      <Alert tone="warn" title="This file is large">
        It is larger than {{ formatBytes(limit) }}, so it was not downloaded.
        <template v-if="file.data.value?.size">
          Its size is {{ formatBytes(file.data.value.size) }}.</template
        >
        <div class="row">
          <Button v-if="canRaise" size="sm" @click="limit = HARD_LIMIT"
            >Load up to {{ formatBytes(HARD_LIMIT) }}</Button
          >
          <span v-else class="faint"
            >It exceeds the viewer’s {{ formatBytes(HARD_LIMIT) }} ceiling; fetch it with the
            CLI.</span
          >
        </div>
      </Alert>
    </template>
    <template v-else-if="content">
      <div class="meta">
        <span class="faint"
          >{{ formatBytes(content.size)
          }}<template v-if="!content.binary">
            ·
            {{ content.text.split('\n').length.toLocaleString('en-US') }}
            lines</template
          ></span
        >
        <span class="spacer" />
        <Segmented
          v-if="!content.binary && !editing"
          v-model="view"
          legend="View"
          :options="VIEWS"
        />
        <RouterLink :to="{ path: `/repos/${repo}/history`, query: { rev, path } }" class="hist"
          >File history</RouterLink
        >
        <Button
          v-if="canEdit && !content.binary && !editing"
          size="sm"
          icon="edit"
          data-testid="edit-file"
          @click="editing = true"
          >{{ hasDraft ? 'Resume draft' : 'Edit' }}</Button
        >
        <FileActions
          v-if="canEdit && !editing && file.data.value?.entry"
          :repo="repo"
          :rev="file.data.value.revision"
          :stream="rev"
          :entry="file.data.value.entry"
        />
        <Button size="sm" icon="download" @click="download">{{
          editing ? 'Download original' : 'Download'
        }}</Button>
      </div>
      <FileEditor
        v-if="editing && !content.binary"
        :repo="repo"
        :path="path"
        :original="content.text"
        :rev="file.data.value!.revision"
        :stream="rev"
        :executable="file.data.value!.executable"
        :file-type="file.data.value!.entry?.file_type"
        @cancel="editing = false"
      />
      <figure v-else-if="image" class="img">
        <img :src="image" :alt="`Preview of ${baseName(path)}`" />
      </figure>
      <Alert v-else-if="content.binary" tone="info" title="Binary file">
        This looks like binary data ({{ formatBytes(content.size) }}), so it is not shown as text.
      </Alert>
      <p v-else-if="content.text === ''" class="faint">This file is empty.</p>
      <BlameTable
        v-else-if="view === 'blame'"
        :repo="repo"
        :rev="file.data.value!.revision"
        :path="path"
      />
      <CodeBlock v-else :code="content.text" :label="`Contents of ${path}`" max-height="70vh" />
    </template>
  </div>
</template>

<style scoped>
.viewer {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--s-3);
  font-size: var(--fs-sm);
}
.spacer {
  flex: 1;
}
.row {
  margin-top: var(--s-2);
}
.img {
  margin: 0;
  padding: var(--s-3);
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  text-align: center;
}
.img img {
  max-width: 100%;
  max-height: 60vh;
}
</style>
