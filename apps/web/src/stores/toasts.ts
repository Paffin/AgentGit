import { defineStore } from 'pinia'
import { ref } from 'vue'
import { describeError, isApiError } from '@/api/errors'

export type ToastTone = 'info' | 'ok' | 'warn' | 'bad'

export interface Toast {
  id: number
  tone: ToastTone
  title: string
  message: string
  /** Request id of a failed call, so it can be quoted in a bug report. */
  requestId: string
  timeoutMs: number
}

let nextId = 1

export const useToasts = defineStore('toasts', () => {
  const items = ref<Toast[]>([])
  const timers = new Map<number, ReturnType<typeof setTimeout>>()

  function dismiss(id: number) {
    const t = timers.get(id)
    if (t !== undefined) clearTimeout(t)
    timers.delete(id)
    items.value = items.value.filter((x) => x.id !== id)
  }

  function arm(toast: Toast) {
    if (toast.timeoutMs <= 0) return
    timers.set(
      toast.id,
      setTimeout(() => dismiss(toast.id), toast.timeoutMs),
    )
  }

  function push(
    tone: ToastTone,
    title: string,
    message = '',
    opts: { requestId?: string; timeoutMs?: number } = {},
  ) {
    const toast: Toast = {
      id: nextId++,
      tone,
      title,
      message,
      requestId: opts.requestId ?? '',
      timeoutMs: opts.timeoutMs ?? (tone === 'bad' ? 9000 : tone === 'warn' ? 7000 : 4500),
    }
    items.value = [...items.value.slice(-4), toast]
    arm(toast)
    return toast.id
  }

  /** Pause auto-dismissal while the pointer or keyboard focus is on the toast. */
  function pause(id: number) {
    const t = timers.get(id)
    if (t !== undefined) clearTimeout(t)
    timers.delete(id)
  }
  function resume(id: number) {
    const toast = items.value.find((x) => x.id === id)
    if (toast && !timers.has(id)) arm(toast)
  }

  return {
    items,
    dismiss,
    pause,
    resume,
    info: (title: string, message?: string) => push('info', title, message),
    ok: (title: string, message?: string) => push('ok', title, message),
    warn: (title: string, message?: string) => push('warn', title, message),
    error(title: string, err?: unknown) {
      return push('bad', title, err === undefined ? '' : describeError(err), {
        requestId: isApiError(err) ? err.requestId : '',
      })
    },
    clear() {
      for (const t of timers.values()) clearTimeout(t)
      timers.clear()
      items.value = []
    },
  }
})
