<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { proposeChange } from '@/lib/edit'
import { checkUploads, uploadChanges, uploadFiles, type CheckedUpload } from '@/lib/uploads'
import { formatBytes } from '@/lib/format'
import { useResource } from '@/composables/useResource'
import { useUnsavedChanges } from '@/composables/useUnsavedChanges'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Button from '@/ui/Button.vue'
import TextField from '@/ui/TextField.vue'
import TextArea from '@/ui/TextArea.vue'
import Alert from '@/ui/Alert.vue'
import Checkbox from '@/ui/Checkbox.vue'
import Dialog from '@/ui/Dialog.vue'
import ErrorState from '@/ui/ErrorState.vue'

const props = defineProps<{ repo: string; stream: string; path: string }>()
const emit = defineEmits<{ cancel: [] }>()
const router = useRouter()
const board = useBoard()
const toasts = useToasts()
const files = ref<File[]>([])
const folder = ref(props.path)
const title = ref('')
const description = ref('')
const error = ref('')
const busy = ref(false)
const done = ref(false)
const reviewed = ref<CheckedUpload[] | null>(null)
const replace = ref(false)
const suggestedTitle = (count: number) => `Upload ${count} ${count === 1 ? 'file' : 'files'}`
const base = useResource(async () => (await api.streams.head(props.repo, props.stream)).head)
const selected = computed(() => {
  try {
    return { files: uploadFiles(files.value, folder.value), error: '' }
  } catch (error) {
    return { files: [], error: describeError(error) }
  }
})
const replacing = computed(() => reviewed.value?.filter((file) => file.replaces) ?? [])
const ready = computed(
  () =>
    files.value.length > 0 &&
    !selected.value.error &&
    !busy.value &&
    base.settled.value &&
    !base.error.value,
)
const { askingToLeave, requestLeave, decide } = useUnsavedChanges(
  computed(() => files.value.length > 0 && !done.value),
  busy,
)
watch(
  () => files.value.length,
  (count, previous) => {
    if (!title.value || title.value === suggestedTitle(previous))
      title.value = suggestedTitle(count)
  },
)
watch(
  [files, folder],
  () => {
    reviewed.value = null
    replace.value = false
    error.value = ''
  },
  { deep: true },
)
function choose(values: FileList | null) {
  files.value = values ? Array.from(values) : []
}
function retryBase() {
  if (!base.loading.value) void base.reload()
}
async function cancel() {
  if (await requestLeave()) emit('cancel')
}
async function submit() {
  if (!ready.value) return
  busy.value = true
  error.value = ''
  try {
    if (!reviewed.value) {
      reviewed.value = await checkUploads(
        api,
        props.repo,
        base.data.value ?? '',
        selected.value.files,
      )
      return
    }
    if (replacing.value.length && !replace.value) return
    const proposal = await proposeChange(api, {
      repo: props.repo,
      stream: props.stream,
      fromRev: base.data.value || undefined,
      expectedBase: base.data.value,
      title: title.value.trim(),
      description: description.value.trim() || undefined,
      changes: await uploadChanges(reviewed.value),
    })
    done.value = true
    busy.value = false
    board.touched(['proposals', 'streams', 'predict'])
    toasts.ok('Change request opened', proposal.title)
    await router.push({
      name: 'proposal',
      params: { owner: props.repo.split('/')[0], name: props.repo.split('/')[1], id: proposal.id },
    })
  } catch (e) {
    error.value = describeError(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <form class="upload" @submit.prevent="submit">
    <div
      class="drop"
      @dragover.prevent
      @drop.prevent="!busy && choose($event.dataTransfer?.files ?? null)"
    >
      <label for="upload-files">Files to upload</label>
      <input
        id="upload-files"
        type="file"
        multiple
        :disabled="busy"
        @change="choose(($event.target as HTMLInputElement).files)"
      />
      <p class="faint">
        Choose files or drop them here. Up to 100 files and 4 MiB per change request.
      </p>
    </div>
    <TextField
      v-model="folder"
      label="Destination folder"
      :disabled="busy"
      placeholder="Repository root"
      hint="Leave empty to upload at the root of this branch."
      mono
    />
    <Alert v-if="selected.error || error" tone="bad" live="alert" title="Upload needs attention">{{
      selected.error || error
    }}</Alert>
    <ErrorState
      v-if="base.error.value"
      what="the upload branch"
      :error="base.error.value"
      :retryable="false"
      compact
    >
      <Button :loading="base.loading.value" :disabled="base.loading.value" @click="retryBase">
        Retry branch
      </Button>
    </ErrorState>
    <ul v-if="selected.files.length" class="files" aria-label="Files in this upload">
      <li v-for="(item, index) in selected.files" :key="item.path">
        <span class="mono">{{ item.path }}</span
        ><span class="faint">{{ formatBytes(item.file.size) }}</span>
        <Button
          size="sm"
          :disabled="busy"
          :label="`Remove ${item.path}`"
          @click="files.splice(index, 1)"
          >Remove</Button
        >
      </li>
    </ul>
    <Alert v-if="reviewed" tone="info" title="Review before opening the change request">
      <p v-if="replacing.length">
        {{ replacing.length }} existing {{ replacing.length === 1 ? 'file will' : 'files will' }} be
        replaced.
      </p>
      <p v-else>All files are new. Existing files will be kept.</p>
      <ul v-if="replacing.length">
        <li v-for="file in replacing" :key="file.path" class="mono">{{ file.path }}</li>
      </ul>
      <Checkbox
        v-if="replacing.length"
        v-model="replace"
        label="Replace the existing files listed above"
      />
      <p>The branch stays unchanged until the change request is reviewed and landed.</p>
    </Alert>
    <TextField v-model="title" label="Change request title" required :disabled="busy" />
    <TextArea v-model="description" label="Description (optional)" :rows="3" :disabled="busy" />
    <div class="buttons">
      <Button :disabled="busy" @click="cancel">Cancel</Button>
      <Button
        type="submit"
        variant="primary"
        :loading="busy"
        :disabled="!ready || !title.trim() || (!!reviewed && replacing.length > 0 && !replace)"
        >{{ reviewed ? 'Open change request' : 'Review upload' }}</Button
      >
    </div>
  </form>
  <Dialog
    :model-value="askingToLeave"
    title="Discard this upload?"
    description="Your files have not been sent as a change request. The originals stay on your computer."
    size="sm"
    alert
    @update:model-value="decide(false)"
  >
    <template #footer
      ><Button variant="danger" @click="decide(true)">Discard upload</Button
      ><Button variant="primary" data-autofocus @click="decide(false)"
        >Keep editing</Button
      ></template
    >
  </Dialog>
</template>

<style scoped>
.upload {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.drop {
  border: 1px dashed var(--border-strong);
  border-radius: var(--r-sm);
  padding: var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.drop label {
  font-weight: 600;
}
.files {
  margin: 0;
  padding: 0;
  list-style: none;
}
.files li {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: var(--s-3);
  padding: var(--s-2) 0;
  border-bottom: 1px solid var(--border);
}
.files li > :first-child {
  flex: 1;
  overflow-wrap: anywhere;
  min-width: 0;
}
.buttons {
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: var(--s-2);
}
</style>
