<script setup lang="ts">
import { nextTick, ref } from 'vue'
import { useAuth } from '@/stores/auth'
import { useFileDraft } from '@/composables/useFileDraft'
import FileEditor from './FileEditor.vue'

const props = defineProps<{ repo: string; stream: string; path: string; enabled: boolean }>()
const emit = defineEmits<{ cancel: [] }>()
const draft = useFileDraft({
  identity: useAuth().identity,
  repo: props.repo,
  stream: props.stream,
  path: props.path,
  isNew: false,
})
const saved = draft.initial
const dismissed = ref(false)
async function leave() {
  // Remove this editor's navigation guard before returning to the folder.
  dismissed.value = true
  await nextTick()
  emit('cancel')
}
</script>

<template>
  <div v-if="enabled && saved && !dismissed" class="recovery">
    <p class="missing">
      This file was removed or moved. Proposing your recovered draft opens a review against the
      current branch.
    </p>
    <FileEditor
      :repo="repo"
      :stream="stream"
      :rev="stream"
      :path="path"
      :original="saved.original"
      :executable="saved.executable"
      :file-type="saved.fileType"
      @cancel="leave"
    />
  </div>
  <slot v-else />
</template>

<style scoped>
.recovery {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.missing {
  margin: 0;
  color: var(--text-2);
}
</style>
