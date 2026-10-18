<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue'
import { useServerInfo } from '@/stores/server'
import { SUPPORTED_PROTOCOL } from '@/lib/version'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'

/**
 * Says so when this copy of the UI does not belong to the server it talks to: a tab left open across a server
 * upgrade, or a UI directory (`[ui] dir`) that was not rebuilt. Both versions are read from the real thing:
 * the UI's from the build it came from, the server's from `GET /v1/capabilities`.
 */
const info = useServerInfo()

function recheck() {
  if (document.visibilityState === 'visible') void info.check()
}
onMounted(() => {
  void info.check(true)
  document.addEventListener('visibilitychange', recheck)
  window.addEventListener('online', recheck)
})
onBeforeUnmount(() => {
  document.removeEventListener('visibilitychange', recheck)
  window.removeEventListener('online', recheck)
})

function reload() {
  window.location.reload()
}
</script>

<template>
  <div
    v-if="info.badProtocol || info.fit.kind === 'mismatch'"
    class="version"
    data-testid="version-banner"
  >
    <Alert
      v-if="info.badProtocol"
      tone="bad"
      live="status"
      title="This page cannot talk to this server"
    >
      The server selected the protocol {{ info.badProtocol }}; this page speaks
      {{ SUPPORTED_PROTOCOL }}. Reloading may fetch a matching page.
      <Button size="sm" data-testid="version-reload" @click="reload">Reload</Button>
    </Alert>
    <Alert
      v-else-if="info.fit.kind === 'mismatch'"
      tone="warn"
      live="status"
      title="This page and the server are different versions"
    >
      This page is version {{ info.fit.ui }}; the server runs {{ info.fit.server }}.
      <template v-if="info.fit.uiNewer">
        The server is older than the page: some screens may ask for things it does not have yet.
      </template>
      <template v-else>
        The server was updated after this page loaded: reload to get the matching screens. If this
        stays after a reload, the UI the server serves is out of date and an administrator should
        rebuild it.
      </template>
      <Button size="sm" data-testid="version-reload" @click="reload">Reload</Button>
    </Alert>
  </div>
</template>

<style scoped>
.version {
  max-width: var(--content-max);
  margin: var(--s-3) auto 0;
  padding: 0 var(--s-4);
}
</style>
