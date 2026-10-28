<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import { usePageTitle } from '@/composables/usePageTitle'
import { PRESETS, applyPreset, describePolicy, stringifyDoc } from '@/lib/policy-plain'
import type { PresetId } from '@/lib/policy-plain'
import { useAuth } from '@/stores/auth'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import EmptyState from '@/ui/EmptyState.vue'
import Icon from '@/ui/Icon.vue'
import TextArea from '@/ui/TextArea.vue'
import TextField from '@/ui/TextField.vue'

usePageTitle(() => 'Start a repository')
const auth = useAuth()
const router = useRouter()
const toasts = useToasts()

// A page (not a dialog) does not focus its first field by itself.
onMounted(
  () => void nextTick(() => document.querySelector<HTMLElement>('[data-autofocus]')?.focus()),
)

type Step = 'name' | 'rules' | 'review'
const STEPS: { id: Step; label: string }[] = [
  { id: 'name', label: 'Name it' },
  { id: 'rules', label: 'Choose the rules' },
  { id: 'review', label: 'Create' },
]
const step = ref<Step>('name')
const stepIndex = computed(() => STEPS.findIndex((s) => s.id === step.value))

const name = ref('')
const description = ref('')
const preset = ref<PresetId>('team')
const busy = ref(false)
const error = ref<ApiError | null>(null)
const canCreate = computed(() => auth.status === 'ready' && auth.isAdminAnywhere)
let context = 0
let owner = auth.identity
watch([() => auth.identity, canCreate], ([identity]) => {
  context++
  busy.value = false
  error.value = null
  if (owner !== identity) {
    name.value = ''
    description.value = ''
    preset.value = 'team'
    step.value = 'name'
  }
  owner = identity
})
onBeforeUnmount(() => {
  context++
})

const NAME = /^[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._-]*$/
const nameProblem = computed(() => {
  const n = name.value.trim()
  if (!n) return ''
  if (!n.includes('/')) return 'Write it as owner/name, for example acme/webapp.'
  if (!NAME.test(n))
    return 'Use lowercase letters, digits, dot, dash and underscore, as owner/name.'
  return ''
})
const canNext = computed(() => name.value.trim() !== '' && !nameProblem.value)
const chosen = computed(() => PRESETS.find((p) => p.id === preset.value) ?? PRESETS[0]!)
const meaning = computed(() => describePolicy(applyPreset(preset.value)))
const canSubmit = computed(
  () => canCreate.value && canNext.value && !error.value?.isUnauthenticated,
)

function next() {
  if (step.value === 'name' && canNext.value) step.value = 'rules'
  else if (step.value === 'rules') step.value = 'review'
}
function back() {
  if (step.value === 'review') step.value = 'rules'
  else if (step.value === 'rules') step.value = 'name'
}

function captureCreation() {
  return {
    context,
    input: { name: name.value.trim(), description: description.value.trim() },
    policy: stringifyDoc(applyPreset(preset.value)),
    policyName: chosen.value.name,
    options: { token: auth.token, quietUnauthenticated: true },
  }
}

async function saveRules(
  repo: { name: string; default_stream: string },
  request: ReturnType<typeof captureCreation>,
) {
  try {
    await api.admin.setPolicy(repo.name, repo.default_stream, request.policy, request.options)
    if (request.context !== context) return
    toasts.ok('Repository created', `${repo.name}, with the “${request.policyName}” rules.`)
  } catch (e) {
    if (request.context !== context) return
    toasts.warn(
      'Repository created, but the rules were not saved',
      `${repo.name}: ${describeError(e)} Set them under Settings.`,
    )
  }
}

function reportFailure(cause: unknown, mine: number, createdName: string) {
  if (mine !== context) return
  error.value =
    cause instanceof ApiError
      ? cause
      : new ApiError({ status: 0, code: 'internal', message: describeError(cause) })
  if (!createdName) step.value = 'name'
}

async function create() {
  if (busy.value || !canSubmit.value) return
  const request = captureCreation()
  busy.value = true
  error.value = null
  let repoName = ''
  try {
    const repo = await api.repos.create(request.input, request.options)
    repoName = repo.name
    if (request.context !== context) return
    await saveRules(repo, request)
    if (request.context !== context) return
    const [owner = '', rname = ''] = repo.name.split('/')
    await router.push({ name: 'board', params: { owner, name: rname }, query: { welcome: '1' } })
  } catch (e) {
    reportFailure(e, request.context, repoName)
  } finally {
    if (request.context === context) busy.value = false
  }
}
</script>

<template>
  <div class="page">
    <div>
      <p class="crumb"><RouterLink to="/repos">Repositories</RouterLink> / New</p>
      <h1>Start a repository</h1>
      <p class="muted">Two questions, then you are in. You can change both later.</p>
      <p>
        Coming from GitHub or GitLab?
        <RouterLink to="/import">Import an existing repository</RouterLink>.
      </p>
    </div>

    <Card v-if="!canCreate" flush>
      <EmptyState
        icon="lock"
        title="Starting a repository needs the admin role"
        description="Ask an administrator to create it, or to give you the admin role. You can still open any repository you have a role on."
      >
        <Button to="/repos">Browse repositories</Button>
      </EmptyState>
    </Card>

    <template v-else>
      <Alert v-if="error" tone="bad" live="alert" title="Could not create the repository">
        {{ error.message }}
        <Button
          v-if="error.isUnauthenticated"
          :to="{ path: '/signin', query: { next: '/repos/new' } }"
          >Sign in again</Button
        >
      </Alert>
      <ol class="steps" aria-label="Progress" data-testid="new-repo-steps">
        <li
          v-for="(s, i) in STEPS"
          :key="s.id"
          :class="{ done: i < stepIndex, current: i === stepIndex }"
          :aria-current="i === stepIndex ? 'step' : undefined"
        >
          <span class="n" aria-hidden="true"
            ><Icon v-if="i < stepIndex" name="check" :size="12" /><template v-else>{{
              i + 1
            }}</template></span
          >
          {{ s.label }}
          <span v-if="i < stepIndex" class="sr-only">(done)</span>
        </li>
      </ol>

      <Card v-if="step === 'name'" title="Name it" icon="database">
        <form class="form" @submit.prevent="next">
          <TextField
            v-model="name"
            label="Name"
            mono
            required
            placeholder="acme/webapp"
            :error="nameProblem || undefined"
            hint="Owner and project, like acme/webapp. The owner is usually your team."
            autofocus
          />
          <TextArea
            v-model="description"
            label="What lives here (optional)"
            :rows="2"
            placeholder="Customer-facing web app"
          />
          <div class="nav">
            <Button to="/repos">Cancel</Button>
            <Button type="submit" variant="primary" icon-right="arrow-right" :disabled="!canNext"
              >Next: choose the rules</Button
            >
          </div>
        </form>
      </Card>

      <Card
        v-else-if="step === 'rules'"
        title="How strict should it be?"
        icon="shield"
        subtitle="The rules decide who may land changes and what must happen first. Pick the closest; you can fine-tune later under Settings."
      >
        <form class="form" @submit.prevent="next">
          <fieldset class="presets">
            <legend class="sr-only">Rules</legend>
            <label
              v-for="p in PRESETS"
              :key="p.id"
              class="preset"
              :class="{ on: preset === p.id }"
              :data-testid="`new-preset-${p.id}`"
            >
              <input v-model="preset" type="radio" name="preset" :value="p.id" />
              <span class="pname">{{ p.name }}</span>
              <span class="pfor">{{ p.forWhom }}</span>
              <span class="psum">{{ p.summary }}</span>
            </label>
          </fieldset>
          <div class="nav">
            <Button @click="back">Back</Button>
            <Button type="submit" variant="primary" icon-right="arrow-right">Next: review</Button>
          </div>
        </form>
      </Card>

      <Card v-else title="Ready to create" icon="check">
        <div class="form">
          <dl class="sum">
            <div>
              <dt>Repository</dt>
              <dd class="mono">{{ name.trim() }}</dd>
            </div>
            <div v-if="description.trim()">
              <dt>About</dt>
              <dd>{{ description.trim() }}</dd>
            </div>
            <div>
              <dt>Rules</dt>
              <dd>{{ chosen.name }}</dd>
            </div>
          </dl>
          <div class="mean">
            <h3>What that means</h3>
            <ul role="list">
              <li v-for="s in meaning" :key="s">{{ s }}</li>
            </ul>
          </div>
          <div class="nav">
            <Button :disabled="busy" @click="back">Back</Button>
            <Button
              variant="primary"
              icon="check"
              :loading="busy"
              :disabled="!canSubmit"
              data-testid="create-repo"
              @click="create"
              >Create repository</Button
            >
          </div>
        </div>
      </Card>
    </template>
  </div>
</template>

<style scoped>
.page {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
  max-width: 820px;
}
.crumb {
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.steps {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.steps li {
  display: inline-flex;
  align-items: center;
  gap: var(--s-2);
  color: var(--text-2);
  font-weight: 560;
}
.steps li.current {
  color: var(--text);
}
.n {
  display: inline-grid;
  place-items: center;
  width: 1.5rem;
  height: 1.5rem;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  font-size: var(--fs-xs);
}
.current .n {
  background: var(--accent);
  border-color: var(--accent);
  color: var(--accent-fg);
}
.done .n {
  background: var(--ok-soft, var(--surface-2));
  border-color: var(--ok-line, var(--ok));
  color: var(--ok-text, var(--ok));
}
.form {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.nav {
  display: flex;
  justify-content: space-between;
  gap: var(--s-3);
  flex-wrap: wrap;
}
fieldset {
  border: 0;
  margin: 0;
  padding: 0;
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: var(--s-3);
}
.preset {
  position: relative;
  display: grid;
  gap: var(--s-1);
  align-content: start;
  padding: var(--s-3);
  border: 1px solid var(--border-strong);
  border-radius: var(--r-md);
  background: var(--surface);
  cursor: pointer;
}
.preset.on {
  border-color: var(--accent);
  box-shadow: 0 0 0 1px var(--accent);
  background: var(--accent-soft, var(--surface-2));
}
.preset:focus-within {
  outline: 2px solid var(--focus);
  outline-offset: 2px;
}
.preset input {
  position: absolute;
  opacity: 0;
  pointer-events: none;
}
.pname {
  font-weight: 650;
}
.pfor {
  font-size: var(--fs-sm);
}
.psum {
  font-size: var(--fs-xs);
  color: var(--text-2);
}
.sum {
  display: grid;
  gap: var(--s-2);
  margin: 0;
}
.sum div {
  display: grid;
  grid-template-columns: 8rem minmax(0, 1fr);
  gap: var(--s-3);
}
.sum dt {
  color: var(--text-2);
}
.sum dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.mean {
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-3) var(--s-4);
}
.mean h3 {
  font-size: var(--fs-md, 1rem);
  margin: 0 0 var(--s-2);
}
.mean ul {
  margin: 0;
  padding-left: var(--s-4);
  display: grid;
  gap: var(--s-1);
  font-size: var(--fs-sm);
}
</style>
