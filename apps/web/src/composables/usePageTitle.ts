import { onBeforeUnmount, watchEffect } from 'vue'

/** Sets the document title (and so the screen-reader announcement) from a reactive value. */
export function usePageTitle(title: () => string) {
  const stop = watchEffect(() => {
    const t = title()
    if (t) document.title = `${t} · AgentGit`
  })
  onBeforeUnmount(stop)
}
