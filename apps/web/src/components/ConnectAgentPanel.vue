<script setup lang="ts">
import { computed, ref } from 'vue'
import { connectSteps, subjectProblem } from '@/lib/onboarding'
import Card from '@/ui/Card.vue'
import CopyButton from '@/ui/CopyButton.vue'
import TextField from '@/ui/TextField.vue'
import CommandBlock from './CommandBlock.vue'

/**
 * "Connect your first agent": the commands that take an empty repository to a first proposal, with this server's
 * address and this repository filled in. They are built by `lib/onboarding.ts` and run against a real server by
 * e2e/onboarding.spec.ts, so what is shown here works.
 */
const props = defineProps<{
  /** `owner/name`. */
  repo: string
  /** Whether the viewer is an administrator of the repository (steps 1 and 2 are theirs). */
  admin: boolean
}>()

const subject = ref('agent:my-agent')
const problem = computed(() => subjectProblem(subject.value))
const steps = computed(() =>
  connectSteps({ origin: window.location.origin, repo: props.repo, subject: subject.value }),
)
</script>

<template>
  <Card
    title="Connect your first agent"
    icon="bot"
    subtitle="Nothing has been pushed to this repository yet"
    data-testid="connect-agent"
  >
    <div class="body">
      <p class="intro">
        An agent is just a client with its own identity and a short-lived, scoped credential. Three
        steps take it from nothing to a proposal on this board.
        <template v-if="!admin">
          Steps 1 and 2 need an administrator of this repository; send them this page.
        </template>
      </p>
      <TextField
        v-model="subject"
        label="Agent identity"
        mono
        :error="problem ?? undefined"
        hint="Who the agent is in the audit log and in role grants."
      />
      <ol v-if="steps" class="steps">
        <li v-for="(step, i) in steps" :key="step.id" :data-testid="`connect-step-${step.id}`">
          <div class="row">
            <h3 class="step-title">
              <span class="n" aria-hidden="true">{{ i + 1 }}</span> {{ step.title }}
            </h3>
            <CopyButton :text="step.lines.join('\n')" :label="`Copy step ${i + 1}`" with-label />
          </div>
          <p class="note">{{ step.note }}</p>
          <CommandBlock
            :lines="step.lines"
            :label="`Commands of step ${i + 1}`"
            :testid="`connect-cmd-${step.id}`"
          />
        </li>
      </ol>
      <p v-if="steps" class="faint small">
        Set <code>AG_ADMIN_TOKEN</code> to an administrator’s token for steps 1 and 2. Nothing here
        is sent anywhere: the page only prints commands for you to run.
      </p>
    </div>
  </Card>
</template>

<style scoped>
.body {
  display: grid;
  gap: var(--s-4);
}
.intro {
  margin: 0;
  color: var(--text-2);
}
.steps {
  list-style: none;
  margin: 0;
  padding: 0;
  display: grid;
  gap: var(--s-4);
}
.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.step-title {
  margin: 0;
  font-size: var(--fs-md, 1rem);
  display: flex;
  align-items: center;
  gap: var(--s-2);
}
.n {
  display: inline-grid;
  place-items: center;
  width: 1.5rem;
  height: 1.5rem;
  border-radius: 50%;
  background: var(--accent-soft, var(--surface-2));
  color: var(--accent-text, var(--text));
  font-size: 0.8rem;
}
.note {
  margin: var(--s-1) 0 var(--s-2);
  color: var(--text-2);
}
.small {
  margin: 0;
}
</style>
