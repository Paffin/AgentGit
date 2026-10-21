<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import Card from '@/ui/Card.vue'
import Icon from '@/ui/Icon.vue'

/**
 * The checklist of a new repository: what is done, and for the next step the button that does it. It reads what
 * the board already knows, so it ticks itself as things happen.
 */
const props = defineProps<{ repo: string }>()
const board = useBoard()
const auth = useAuth()

const base = computed(() => `/repos/${props.repo}`)
const isAdmin = computed(() => auth.hasRole(props.repo, 'admin'))
const canWrite = computed(() => auth.can(props.repo, 'editFiles'))

interface Item {
  id: string
  label: string
  done: boolean
  help: string
  action?: { label: string; to: string }
}

const items = computed<Item[]>(() => {
  const worked =
    board.tasks.length > 0 || board.runs.length > 0 || (board.repo?.changeset_count ?? 0) > 0
  const proposed = board.cards.length > 0 || board.recent.length > 0
  const landed = board.recent.length > 0
  return [
    { id: 'created', label: 'Repository created', done: true, help: '' },
    {
      id: 'rules',
      label: 'Rules chosen',
      done: (board.policy?.version ?? 0) > 0,
      help: 'Who may land changes, and what must happen first. Until you choose, the built-in default applies.',
      action: isAdmin.value ? { label: 'Choose the rules', to: `${base.value}/admin` } : undefined,
    },
    {
      id: 'work',
      label: 'Work started',
      done: worked,
      help: 'Give an agent a task, or add a file yourself.',
      action: { label: 'Give an agent work', to: `${base.value}/agents` },
    },
    {
      id: 'proposed',
      label: 'A change request is open',
      done: proposed,
      help: 'When an agent or a person proposes a change, it appears here to be reviewed.',
      action: canWrite.value ? { label: 'Edit a file', to: `${base.value}/files` } : undefined,
    },
    {
      id: 'landed',
      label: 'First change landed',
      done: landed,
      help: 'Review the change request, then land it. The branch moves.',
    },
  ]
})
const next = computed(() => items.value.find((i) => !i.done))
const doneCount = computed(() => items.value.filter((i) => i.done).length)
</script>

<template>
  <Card
    title="Getting started"
    icon="flag"
    :subtitle="`${doneCount} of ${items.length} done`"
    data-testid="getting-started"
  >
    <ol class="list">
      <li v-for="i in items" :key="i.id" :class="{ done: i.done, next: i === next }">
        <span class="mark" aria-hidden="true"
          ><Icon :name="i.done ? 'check' : 'circle'" :size="14"
        /></span>
        <div class="body">
          <p class="label">
            {{ i.label }}<span class="sr-only">{{ i.done ? ' (done)' : ' (to do)' }}</span>
          </p>
          <template v-if="i === next">
            <p class="help">{{ i.help }}</p>
            <RouterLink
              v-if="i.action"
              :to="i.action.to"
              class="go"
              :data-testid="`start-${i.id}`"
              >{{ i.action.label }}</RouterLink
            >
            <p v-else-if="i.id === 'rules'" class="help">
              Ask an administrator to choose them under Settings.
            </p>
          </template>
        </div>
      </li>
    </ol>
  </Card>
</template>

<style scoped>
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: var(--s-3);
}
.list li {
  display: flex;
  gap: var(--s-3);
  align-items: flex-start;
}
.mark {
  display: inline-grid;
  place-items: center;
  flex: none;
  width: 1.5rem;
  height: 1.5rem;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  color: var(--text-3);
}
.done .mark {
  background: var(--ok-soft, var(--surface-2));
  border-color: var(--ok-line, var(--ok));
  color: var(--ok-text, var(--ok));
}
.next .mark {
  border-color: var(--accent);
  color: var(--accent-text, var(--accent));
}
.label {
  font-weight: 560;
}
.done .label {
  color: var(--text-2);
}
.help {
  color: var(--text-2);
  font-size: var(--fs-sm);
  margin-top: 2px;
}
.go {
  display: inline-flex;
  align-items: center;
  min-height: var(--touch-min);
  font-weight: 600;
}
</style>
