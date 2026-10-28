import { computed } from 'vue'
import { api } from '@/api/instance'
import { useAuth } from '@/stores/auth'
import { useResource } from './useResource'
import { useServerNow } from './useNow'

/** Registered choices, loaded only when the assignment form is open. */
export function useAssignableAgents(repository: () => string, enabled: () => boolean) {
  const auth = useAuth()
  const now = useServerNow()
  const supported = computed(() => !!auth.capabilities?.features.includes('tasks.assignment'))
  const active = computed(() => supported.value && enabled())
  const agents = useResource(
    (signal) =>
      api.agents.list(repository(), { signal, token: auth.token, quietUnauthenticated: true }),
    { watch: [repository, () => auth.token, active], enabled: () => active.value },
  )
  const options = computed(() => [
    { value: '', label: 'Automatic routing' },
    ...(agents.data.value || [])
      .filter((agent) => !agent.disabled)
      .map((agent) => ({
        value: agent.name,
        label: auth.capabilities?.features.includes('agents.executor_presence')
          ? `${agent.name} · ${agent.executor && agent.executor.expires_at_ms > now.value ? 'executor connected' : 'no recent executor report'}`
          : agent.name,
      })),
  ])
  return { supported, agents, options }
}
