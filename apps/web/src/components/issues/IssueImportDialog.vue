<script setup lang="ts">
import { computed, onBeforeUnmount, ref, shallowRef, watch } from 'vue'
import { api } from '@/api/instance'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Checkbox from '@/ui/Checkbox.vue'
import Dialog from '@/ui/Dialog.vue'

const props = defineProps<{ modelValue: boolean; repo: string }>()
const emit = defineEmits<{ 'update:modelValue': [open: boolean]; imported: [] }>()
const auth = useAuth()
const toasts = useToasts()
type Preview = Awaited<ReturnType<typeof api.issues.previewImport>>
const manifest = ref('')
const filename = ref('')
const preview = shallowRef<Preview | null>(null)
const allowUnsupported = ref(false)
const busy = ref<'preview' | 'import' | null>(null)
const error = ref('')
const canImport = computed(
  () =>
    !!preview.value &&
    !preview.value.conflicts.length &&
    (!preview.value.warnings.length || allowUnsupported.value),
)
let generation = 0
let controller: AbortController | undefined
function reset() {
  generation++
  controller?.abort()
  manifest.value = filename.value = error.value = ''
  preview.value = null
  allowUnsupported.value = false
  busy.value = null
}
watch([() => props.modelValue, () => props.repo, () => auth.token], reset, { flush: 'sync' })
onBeforeUnmount(reset)

async function chooseFile(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  reset()
  if (!file) return
  const mine = generation
  try {
    if (file.size > 4 * 1024 * 1024) throw new Error('Choose a manifest no larger than 4 MiB.')
    const raw = await file.text()
    JSON.parse(raw)
    if (mine !== generation) return
    manifest.value = raw
    filename.value = file.name
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  }
}
async function readPreview() {
  if (busy.value || !manifest.value) return
  const mine = generation
  controller = new AbortController()
  busy.value = 'preview'
  error.value = ''
  preview.value = null
  allowUnsupported.value = false
  try {
    const result = await api.issues.previewImport(
      props.repo,
      { manifest_json: manifest.value },
      {
        token: auth.token,
        signal: controller.signal,
        quietUnauthenticated: true,
      },
    )
    if (mine === generation) preview.value = result
  } catch (cause) {
    if (mine === generation) error.value = describeError(cause)
  } finally {
    if (mine === generation) busy.value = null
  }
}
async function applyImport() {
  if (busy.value || !preview.value || !canImport.value) return
  const mine = generation
  controller = new AbortController()
  busy.value = 'import'
  error.value = ''
  try {
    const result = await api.issues.applyImport(
      props.repo,
      {
        manifest_json: manifest.value,
        expected_digest: preview.value.digest,
        allow_unsupported: allowUnsupported.value,
      },
      { token: auth.token, signal: controller.signal, quietUnauthenticated: true },
    )
    if (mine !== generation) return
    toasts.ok('Import complete', `${result.created} created, ${result.skipped} already imported.`)
    emit('imported')
    emit('update:modelValue', false)
  } catch (cause) {
    if (mine === generation) {
      error.value = describeError(cause)
      preview.value = null
    }
  } finally {
    if (mine === generation) busy.value = null
  }
}
</script>

<template>
  <Dialog
    :model-value="modelValue"
    title="Import issues and comments"
    description="Choose a GitHub or GitLab issue manifest, preview the changes, then import them into this repository."
    :dismissable="!busy"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="import-form">
      <label class="file-label"
        >JSON manifest
        <input
          type="file"
          accept="application/json,.json"
          :disabled="!!busy"
          @change="chooseFile"
        />
      </label>
      <p v-if="filename" class="faint">{{ filename }}</p>
      <Alert v-if="error" tone="bad" live="alert">{{ error }}</Alert>
      <div v-if="preview" class="preview">
        <p>
          <strong>{{ preview.issues }} issues · {{ preview.comments }} comments</strong>
        </p>
        <p>{{ preview.create_count }} to create · {{ preview.skip_count }} already imported</p>
        <Alert
          v-if="preview.conflicts.length"
          tone="bad"
          title="Resolve conflicts before importing"
        >
          <details open>
            <summary>{{ preview.conflicts.length }} conflicts</summary>
            <ul class="findings">
              <li v-for="(conflict, index) in preview.conflicts" :key="index">{{ conflict }}</li>
            </ul>
          </details>
        </Alert>
        <template v-if="preview.warnings.length">
          <Alert tone="warn" title="Some source data is unsupported">
            <details open>
              <summary>{{ preview.warnings.length }} warnings</summary>
              <ul class="findings">
                <li v-for="(warning, index) in preview.warnings" :key="index">{{ warning }}</li>
              </ul>
            </details>
          </Alert>
          <Checkbox
            v-model="allowUnsupported"
            label="Continue without the unsupported data listed above"
            :disabled="!!busy"
          />
        </template>
      </div>
    </div>
    <template #footer>
      <Button :disabled="!!busy" @click="emit('update:modelValue', false)">Cancel</Button>
      <Button :disabled="!manifest || !!busy" :loading="busy === 'preview'" @click="readPreview"
        >Preview</Button
      >
      <Button
        v-if="preview"
        variant="primary"
        :disabled="!!busy || !canImport"
        :loading="busy === 'import'"
        @click="applyImport"
        >Import</Button
      >
    </template>
  </Dialog>
</template>

<style scoped>
.import-form,
.preview,
.file-label {
  display: grid;
  gap: var(--s-3);
}
input[type='file'] {
  max-width: 100%;
}
.findings {
  max-height: 14rem;
  overflow: auto;
  overflow-wrap: anywhere;
  padding-left: var(--s-4);
}
</style>
