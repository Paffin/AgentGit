import { onBeforeUnmount, ref, watch, type Ref } from 'vue'
import { onBeforeRouteLeave, onBeforeRouteUpdate } from 'vue-router'

function protectTab(dirty: Ref<boolean>, busy: Ref<boolean>) {
  function beforeUnload(event: BeforeUnloadEvent) {
    event.preventDefault()
    event.returnValue = ''
  }
  watch(
    () => dirty.value || busy.value,
    (protect) => {
      if (protect) window.addEventListener('beforeunload', beforeUnload)
      else window.removeEventListener('beforeunload', beforeUnload)
    },
    { immediate: true },
  )
  onBeforeUnmount(() => window.removeEventListener('beforeunload', beforeUnload))
}

/** Keep route changes and tab closes from silently discarding an editor's work. */
export function useUnsavedChanges(dirty: Ref<boolean>, busy: Ref<boolean>) {
  protectTab(dirty, busy)
  const askingToLeave = ref(false)
  let resolveLeave: ((leave: boolean) => void) | undefined

  function decide(leave: boolean) {
    askingToLeave.value = false
    resolveLeave?.(leave)
    resolveLeave = undefined
  }

  function requestLeave(): boolean | Promise<boolean> {
    if (busy.value) return false
    if (!dirty.value) return true
    resolveLeave?.(false)
    askingToLeave.value = true
    return new Promise<boolean>((resolve) => (resolveLeave = resolve))
  }

  onBeforeRouteLeave(requestLeave)
  onBeforeRouteUpdate((to, from) =>
    to.path === from.path && to.query.path === from.query.path && to.query.rev === from.query.rev
      ? true
      : requestLeave(),
  )
  onBeforeUnmount(() => {
    decide(false)
  })
  return { askingToLeave, requestLeave, decide }
}
