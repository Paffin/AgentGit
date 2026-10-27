<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { describeError } from '@/api/errors'
import { randomHex } from '@/api/client'
import {
  bundleCommands,
  finishMigration,
  migrationWarnings,
  prepareMigration,
  readMigration,
} from '@/api/migration'
import type { MigrationPlan } from '@/api/migration'
import { usePageTitle } from '@/composables/usePageTitle'
import { useAuth } from '@/stores/auth'
import Alert from '@/ui/Alert.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import TextField from '@/ui/TextField.vue'

usePageTitle(() => 'Import a repository')
const auth = useAuth()
const source = ref('')
const name = ref('')
const file = ref<File | null>(null)
const key = ref(randomHex())
const plan = ref<MigrationPlan | null>(null)
const busy = ref(false)
const error = ref('')
const copied = ref(false)
const accept = ref(false)
const RESUME_KEY = 'agentgit.migration'
function remember(id: string | null) {
  try {
    if (id) sessionStorage.setItem(RESUME_KEY, id)
    else sessionStorage.removeItem(RESUME_KEY)
  } catch {
    /* session storage may be disabled */
  }
}
onMounted(async () => {
  let id: string | null
  try {
    id = sessionStorage.getItem(RESUME_KEY)
  } catch {
    return
  }
  if (!id) return
  busy.value = true
  try {
    plan.value = await readMigration(id)
  } catch (e) {
    error.value = `Could not resume the previous import: ${describeError(e)}`
    remember(null)
  } finally {
    busy.value = false
  }
})
const suggestedName = computed(() => {
  if (!commands.value) return ''
  const path = new URL(source.value).pathname
    .replace(/\.git$/, '')
    .split('/')
    .filter(Boolean)
    .slice(-2)
  return path.length === 2
    ? path
        .map((part) =>
          part
            .toLowerCase()
            .replace(/[^a-z0-9._-]/g, '-')
            .replace(/^[^a-z0-9]+/, ''),
        )
        .join('/')
    : ''
})
function startAnother() {
  remember(null)
  plan.value = null
  accept.value = false
  file.value = null
  name.value = ''
  key.value = randomHex()
}
const commands = computed(() => {
  if (!source.value.trim()) return ''
  try {
    return bundleCommands(source.value.trim())
  } catch {
    return ''
  }
})
const sourceError = computed(() =>
  source.value && !commands.value
    ? 'Use an HTTPS or SSH repository URL without a token or password.'
    : '',
)
const validName = computed(() =>
  /^[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._-]*$/.test(name.value.trim()),
)
const warnings = computed(() => (plan.value ? migrationWarnings(plan.value.report) : []))
const imported = computed(() => plan.value?.state === 'imported')
const destination = computed(
  () => '/repos/' + (plan.value?.name ?? '').split('/').map(encodeURIComponent).join('/'),
)
function selectFile(event: Event) {
  const next = (event.target as HTMLInputElement).files?.[0] ?? null
  error.value = ''
  if (next && next.size > 128 * 1024 * 1024) {
    error.value = 'This server accepts bundles up to 128 MiB. Use the CLI for larger repositories.'
    file.value = null
  } else file.value = next
  key.value = randomHex()
}
async function copy() {
  try {
    await navigator.clipboard.writeText(commands.value)
    copied.value = true
  } catch {
    error.value = 'Clipboard access is unavailable. Select and copy the commands below.'
  }
}
async function prepare() {
  if (!file.value || !validName.value) return
  busy.value = true
  error.value = ''
  try {
    plan.value = await prepareMigration(name.value.trim(), file.value, key.value)
    remember(plan.value.id)
  } catch (e) {
    error.value = describeError(e)
  } finally {
    busy.value = false
  }
}
async function publish() {
  if (!plan.value) return
  busy.value = true
  error.value = ''
  try {
    plan.value = await finishMigration(plan.value.id, accept.value)
  } catch (e) {
    error.value = describeError(e)
  } finally {
    busy.value = false
  }
}
function downloadReport() {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(plan.value, null, 2)], { type: 'application/json' }),
  )
  const link = document.createElement('a')
  link.href = url
  link.download = 'agentgit-migration-report.json'
  link.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
</script>

<template>
  <div class="import-page">
    <header>
      <p class="crumb"><RouterLink to="/repos">Repositories</RouterLink> / Import</p>
      <h1>Bring your history with you</h1>
      <p class="intro">
        Move a GitHub, GitLab or other Git repository into AgentGit. Review the result before it
        becomes available to your team.
      </p>
    </header>
    <Alert v-if="!auth.isAdminAnywhere" title="An administrator needs to run the import" tone="warn"
      >Ask an administrator to import the repository or grant you the admin role.</Alert
    >
    <template v-else>
      <ol class="journey" aria-label="Import progress">
        <li :aria-current="!plan ? 'step' : undefined">1. Prepare a copy</li>
        <li :aria-current="plan && !imported ? 'step' : undefined">2. Review the import</li>
        <li :aria-current="imported ? 'step' : undefined">3. Start working</li>
      </ol>
      <Alert v-if="error" tone="bad" title="Import needs attention" live="alert">{{ error }}</Alert>
      <template v-if="!plan">
        <Card
          title="Your source stays yours"
          subtitle="The commands below read your repository. They do not change or delete anything on GitHub or GitLab."
        >
          <div class="stack">
            <TextField
              v-model="source"
              label="Source repository URL (optional)"
              placeholder="https://github.com/team/project.git"
              :error="sourceError || undefined"
              hint="Private repositories work with your existing local Git sign-in. Never paste a token here."
            />
            <template v-if="commands">
              <div class="command-heading">
                <strong>Run in Bash or Git Bash in an empty folder</strong
                ><Button size="sm" @click="copy">{{ copied ? 'Copied' : 'Copy commands' }}</Button>
              </div>
              <pre
                tabindex="0"
                aria-label="Commands to create a Git bundle"
              ><code>{{ commands }}</code></pre>
            </template>
            <p v-else class="muted">
              Already have a full Git clone? Run
              <code>git bundle create repository.bundle --all</code> inside it, or enter a URL to
              get both commands.
            </p>
            <p class="note">
              Your GitHub or GitLab credentials stay on your computer. Only the bundle is uploaded.
              Use a full mirror, not a shallow clone, to include every branch and tag.
            </p>
          </div>
        </Card>
        <Card title="Where should it live?">
          <form class="stack" @submit.prevent="prepare">
            <TextField
              v-model="name"
              label="New AgentGit repository"
              placeholder="team/project"
              required
              hint="Use lowercase owner/name. The destination must not exist yet."
            />
            <Button v-if="suggestedName && !name" @click="name = suggestedName"
              >Use {{ suggestedName }}</Button
            >
            <div class="upload">
              <label for="bundle">Git bundle</label
              ><input
                id="bundle"
                type="file"
                accept=".bundle"
                required
                :disabled="busy"
                @change="selectFile"
              /><span class="muted"
                >Up to 128 MiB. The next step checks the history without publishing it.</span
              >
            </div>
            <div class="actions">
              <Button to="/repos" :disabled="busy">Cancel</Button
              ><Button
                type="submit"
                variant="primary"
                :loading="busy"
                :disabled="!validName || !file"
                >{{ busy ? 'Uploading and checking history…' : 'Check the import' }}</Button
              >
            </div>
            <p v-if="busy" role="status">
              Large histories can take several minutes. Keep this page open.
            </p>
          </form>
        </Card>
      </template>
      <template v-else-if="plan">
        <Alert v-if="imported" tone="ok" title="Your repository is ready" live="status"
          >{{ plan.name }} is available. Your source repository has not been changed.</Alert
        >
        <Card
          :title="imported ? 'Import receipt' : 'Review before publishing'"
          :subtitle="plan.name"
        >
          <div class="stack">
            <dl class="counts">
              <div>
                <dt>Commits</dt>
                <dd>{{ plan.report.commits.toLocaleString() }}</dd>
              </div>
              <div>
                <dt>Branches</dt>
                <dd>{{ plan.report.branches.length }}</dd>
              </div>
              <div>
                <dt>Tags</dt>
                <dd>{{ plan.report.tags.length }}</dd>
              </div>
              <div>
                <dt>Default branch</dt>
                <dd>{{ plan.report.default_branch }}</dd>
              </div>
            </dl>
            <details>
              <summary>Branch and tag mapping</summary>
              <ul class="refs">
                <li v-for="branch in plan.report.branches" :key="'branch-' + branch.name">
                  <strong>{{ branch.name }}</strong
                  ><span> → {{ branch.destination }} stream</span
                  ><code>{{ branch.git_commit }}</code>
                </li>
                <li v-for="tag in plan.report.tags" :key="'tag-' + tag.name">
                  <strong>{{ tag.name }}</strong
                  ><span> → {{ tag.destination }} pin</span><code>{{ tag.git_commit }}</code>
                </li>
              </ul>
            </details>
            <Alert
              v-if="warnings.length"
              tone="warn"
              title="Some content needs a separate migration"
              ><ul>
                <li v-for="warning in warnings" :key="warning">{{ warning }}</li>
              </ul>
              <details v-if="plan.report.skipped_paths.length">
                <summary>Omitted paths</summary>
                <ul>
                  <li v-for="[path, reason] in plan.report.skipped_paths" :key="path + reason">
                    <code>{{ path }}</code
                    >: {{ reason }}
                  </li>
                </ul>
              </details></Alert
            >
            <div class="note">
              <strong>What this transfers</strong>
              <p>
                Git file history, authors, commit messages, branches and commit-based tags. Git IDs
                and signed tag objects stay in the original source bundle; AgentGit changesets have
                their own IDs.
              </p>
              <p>
                GitHub and GitLab issues, pull/merge request discussions, permissions, releases, CI
                settings and external LFS files are not imported. Keep the source available until
                your team has checked the result and moved these separately.
              </p>
              <p>
                The server retains the original bundle with the repository and includes it in
                backups. Save this report to reconcile the imported refs.
              </p>
            </div>
            <label v-if="warnings.length && !imported" class="consent"
              ><input v-model="accept" type="checkbox" />I have reviewed these content limitations
              and accept this import.</label
            >
            <div class="actions">
              <Button @click="downloadReport">Save report</Button>
              <Button v-if="imported" @click="startAnother">Import another repository</Button
              ><Button
                v-if="!imported"
                variant="primary"
                :loading="busy"
                :disabled="warnings.length > 0 && !accept"
                @click="publish"
                >Publish repository</Button
              ><Button v-else variant="primary" :to="destination">Open repository</Button>
            </div>
            <p v-if="!imported" class="muted">
              Only you can see this staged import. Nothing is published until you confirm.
              Unpublished imports expire after 24 hours.
            </p>
          </div>
        </Card>
      </template>
    </template>
  </div>
</template>

<style scoped>
.import-page {
  display: grid;
  gap: var(--s-5);
  max-width: 900px;
}
.crumb,
.intro {
  color: var(--text-2);
}
.intro {
  max-width: 68ch;
  line-height: 1.6;
}
.journey {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-4);
  padding: 0;
  margin: 0;
  list-style: none;
  color: var(--text-2);
}
.journey [aria-current] {
  color: var(--text);
  font-weight: 650;
  text-decoration: underline;
  text-underline-offset: 8px;
  text-decoration-color: var(--accent);
}
.stack {
  display: grid;
  gap: var(--s-4);
}
.command-heading,
.actions {
  display: flex;
  justify-content: space-between;
  gap: var(--s-3);
  align-items: center;
  flex-wrap: wrap;
}
pre {
  overflow-x: auto;
  padding: var(--s-4);
  background: var(--surface-2);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  font-size: var(--fs-sm);
  line-height: 1.8;
  margin: 0;
}
.note {
  padding: var(--s-4);
  background: var(--surface-2);
  border-left: 3px solid var(--border-strong);
  font-size: var(--fs-sm);
  line-height: 1.6;
}
.note p {
  margin: var(--s-2) 0 0;
}
.upload {
  display: grid;
  gap: var(--s-2);
  border: 1px dashed var(--border-strong);
  border-radius: var(--r-md);
  padding: var(--s-4);
}
.upload label {
  font-weight: 600;
}
.upload input {
  max-width: 100%;
  min-height: 44px;
  padding-block: var(--s-2);
}
.counts {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(120px, 1fr));
  gap: var(--s-4);
  margin: 0;
}
dt {
  color: var(--text-2);
  font-size: var(--fs-sm);
}
dd {
  font-size: var(--fs-xl);
  font-weight: 620;
  margin: var(--s-1) 0 0;
  overflow-wrap: anywhere;
}
summary {
  cursor: pointer;
  padding-block: 12px;
}
.refs {
  padding-left: var(--s-4);
}
.refs li {
  padding: var(--s-2) 0;
  overflow-wrap: anywhere;
}
.refs code {
  display: block;
  font-size: var(--fs-xs);
  color: var(--text-2);
}
.consent {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  min-height: 44px;
  cursor: pointer;
}
.consent input {
  width: 20px;
  height: 20px;
  flex-shrink: 0;
  accent-color: var(--accent);
}
@media (max-width: 520px) {
  .journey {
    font-size: var(--fs-sm);
    gap: var(--s-3);
  }
  .actions > * {
    flex: 1;
  }
}
</style>
