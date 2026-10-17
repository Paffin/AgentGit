import { computed } from 'vue'
import { useRoute } from 'vue-router'

/** `owner/name` of the repository the current route belongs to. */
export function useRepoName() {
  const route = useRoute()
  const repo = computed(
    () => `${String(route.params.owner ?? '')}/${String(route.params.name ?? '')}`,
  )
  const routeParams = computed(() => ({
    owner: String(route.params.owner ?? ''),
    name: String(route.params.name ?? ''),
  }))
  return { repo, routeParams }
}
