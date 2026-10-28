<script setup lang="ts">
import { onBeforeUnmount, ref, watch } from 'vue'
import { api } from '@/api/instance'
import type { Agent } from '@/api/types'
import { randomHex } from '@/api/client'
import { describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'
import Button from '@/ui/Button.vue'
import Dialog from '@/ui/Dialog.vue'
import Alert from '@/ui/Alert.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'

const props = defineProps<{ repo: string; agent: Agent }>()
const emit = defineEmits<{ changed: [] }>()
const auth = useAuth()
const open = ref(false),
  busy = ref(false),
  error = ref(''),
  wasDisabled = ref(false)
let generation = 0,
  controller: AbortController | undefined,
  key = randomHex()
function reset() {
  generation++
  controller?.abort()
  open.value = false
  busy.value = false
  error.value = ''
}
watch([() => props.repo, () => props.agent.name, () => auth.token], reset, { flush: 'sync' })
onBeforeUnmount(reset)
function begin() {
  wasDisabled.value = props.agent.disabled
  error.value = ''
  key = randomHex()
  open.value = true
}
async function apply() {
  if (busy.value || !auth.can(props.repo, 'controlExecution')) return
  const mine = generation
  controller = new AbortController()
  busy.value = true
  error.value = ''
  try {
    await api.agents.setState(props.repo, props.agent.name, !wasDisabled.value, wasDisabled.value, {
      signal: controller.signal,
      token: auth.token,
      idempotencyKey: key,
      quietUnauthenticated: true,
    })
    if (mine !== generation) return
    open.value = false
    emit('changed')
  } catch (cause) {
    if (mine === generation) {
      error.value = describeError(cause)
      emit('changed')
    }
  } finally {
    if (mine === generation) busy.value = false
  }
}
</script>

<template>
  <Button
    size="sm"
    :label="`${agent.disabled ? 'Enable' : 'Disable'} agent ${agent.name}`"
    @click="begin"
  >
    {{ agent.disabled ? 'Enable…' : 'Disable…' }}
  </Button>
  <Dialog
    v-model="open"
    :title="wasDisabled ? 'Enable agent?' : 'Disable agent?'"
    :dismissable="!busy"
  >
    <p><IdentityLabel :identity="agent.identity" /></p>
    <p v-if="wasDisabled">
      This agent can start new runs again. Its executor must still be connected and task execution
      must be allowed.
    </p>
    <p v-else>
      New runs for this agent will be refused. Running sessions keep working; cancel their tasks
      separately if they must stop. Its key, role and history are retained.
    </p>
    <Alert v-if="error" tone="bad" live="alert"
      >{{ error }} Close and reopen this dialog to review the current state.</Alert
    >
    <template #footer>
      <Button :disabled="busy" @click="open = false">Keep current state</Button>
      <Button :variant="wasDisabled ? 'primary' : 'danger'" :loading="busy" @click="apply">
        {{ wasDisabled ? 'Enable agent' : 'Disable agent' }}
      </Button>
    </template>
  </Dialog>
</template>
