import { computed, shallowRef } from 'vue'

const stack = shallowRef<symbol[]>([])
let page: { root: HTMLElement | null; inert: boolean; overflow: string } | undefined

function lockPage() {
  const root = document.getElementById('app')
  page = {
    root,
    inert: root?.hasAttribute('inert') ?? false,
    overflow: document.documentElement.style.overflow,
  }
  root?.setAttribute('inert', '')
  document.documentElement.style.overflow = 'hidden'
}

function unlockPage() {
  if (!page) return
  if (!page.inert) page.root?.removeAttribute('inert')
  document.documentElement.style.overflow = page.overflow
  page = undefined
}

/** Dialogs may replace or nest one another without unlocking the page underneath. */
export function modalLayer() {
  const id = Symbol('dialog')
  const top = computed(() => stack.value.at(-1) === id)
  const zIndex = computed(() => `calc(var(--z-dialog) + ${stack.value.indexOf(id) + 1})`)

  function activate() {
    if (stack.value.includes(id)) return
    if (stack.value.length === 0) lockPage()
    stack.value = [...stack.value, id]
  }

  function deactivate() {
    if (!stack.value.includes(id)) return
    stack.value = stack.value.filter((entry) => entry !== id)
    if (stack.value.length === 0) unlockPage()
  }
  return { top, zIndex, activate, deactivate }
}
