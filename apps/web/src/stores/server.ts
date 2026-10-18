import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '@/api/instance'
import { compareBuilds, protocolMismatch } from '@/lib/version'

/** Where the build writes the UI's own version (see vite.config.ts). */
export const UI_VERSION_PATH = '/ui-version.json'
const RECHECK_MS = 60_000

/** Reads the version of the UI build that this page came from. `null` when it cannot be read (dev server). */
export async function fetchUiVersion(): Promise<string | null> {
  try {
    const res = await fetch(UI_VERSION_PATH, {
      cache: 'no-store',
      headers: { accept: 'application/json' },
    })
    if (!res.ok) return null
    const body = (await res.json()) as { version?: unknown }
    return typeof body.version === 'string' ? body.version : null
  } catch {
    return null
  }
}

/** The UI build against the server it talks to, both read from the real thing. */
export const useServerInfo = defineStore('server', () => {
  const uiVersion = ref<string | null>(null)
  const serverVersion = ref<string | null>(null)
  const protocol = ref<string | null>(null)
  let lastCheck = 0
  let inflight: Promise<void> | null = null

  const fit = computed(() => compareBuilds(uiVersion.value, serverVersion.value))
  const badProtocol = computed(() => protocolMismatch(protocol.value))

  async function check(force = false): Promise<void> {
    if (inflight) return inflight
    if (!force && Date.now() - lastCheck < RECHECK_MS && serverVersion.value !== null) return
    inflight = (async () => {
      try {
        const [ui, caps] = await Promise.all([fetchUiVersion(), api.capabilities()])
        uiVersion.value = ui
        serverVersion.value = caps.server_version || null
        protocol.value = caps.protocol_version || null
        lastCheck = Date.now()
      } catch {
        // The server cannot be reached or does not answer: that is the offline page's business, not a banner's.
      } finally {
        inflight = null
      }
    })()
    return inflight
  }

  return { uiVersion, serverVersion, protocol, fit, badProtocol, check }
})
