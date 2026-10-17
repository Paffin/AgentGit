import { defineStore } from 'pinia'
import { ref, watch } from 'vue'
import { readStore, writeStore } from '@/lib/storage'
import { DEFAULT_STALE_AFTER_MS } from '@/lib/runs'

export type ThemeMode = 'system' | 'light' | 'dark'

const THEME_KEY = 'agentgit.theme'
const STALE_KEY = 'agentgit.staleAfterSec'
const MINOR_KEY = 'agentgit.showMinorEvents'

function readTheme(): ThemeMode {
  const v = readStore('local', THEME_KEY)
  return v === 'light' || v === 'dark' ? v : 'system'
}

/** Per-viewer conveniences. Losing them (blocked storage, private window) only resets the defaults. */
export const usePrefs = defineStore('prefs', () => {
  const theme = ref<ThemeMode>(readTheme())
  const staleAfterMs = ref<number>(
    (() => {
      const n = Number(readStore('local', STALE_KEY))
      return Number.isFinite(n) && n >= 5 ? n * 1000 : DEFAULT_STALE_AFTER_MS
    })(),
  )
  const showMinorEvents = ref(readStore('local', MINOR_KEY) === '1')

  function applyTheme() {
    const root = document.documentElement
    if (theme.value === 'system') root.removeAttribute('data-theme')
    else root.setAttribute('data-theme', theme.value)
  }

  watch(theme, (t) => {
    writeStore('local', THEME_KEY, t)
    applyTheme()
  })
  watch(staleAfterMs, (ms) => writeStore('local', STALE_KEY, String(Math.round(ms / 1000))))
  watch(showMinorEvents, (v) => writeStore('local', MINOR_KEY, v ? '1' : '0'))

  function cycleTheme() {
    theme.value = theme.value === 'system' ? 'light' : theme.value === 'light' ? 'dark' : 'system'
  }

  return { theme, staleAfterMs, showMinorEvents, applyTheme, cycleTheme }
})
