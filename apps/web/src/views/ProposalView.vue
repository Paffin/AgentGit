<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api } from '@/api/instance'
import BlockerList from '@/components/BlockerList.vue'
import StateStrip from '@/components/StateStrip.vue'
import ConflictResolver from '@/components/proposal/ConflictResolver.vue'
import ChangesTab from '@/components/proposal/ChangesTab.vue'
import IntegrateDialog from '@/components/proposal/IntegrateDialog.vue'
import ProvenanceTab from '@/components/proposal/ProvenanceTab.vue'
import ReviewDialog from '@/components/proposal/ReviewDialog.vue'
import ReviewsTab from '@/components/proposal/ReviewsTab.vue'
import ProposalComment from '@/components/proposal/ProposalComment.vue'
import ReviewThreads from '@/components/proposal/ReviewThreads.vue'
import RequestedReviewers from '@/components/proposal/RequestedReviewers.vue'
import TimelineTab from '@/components/proposal/TimelineTab.vue'
import ValidationTab from '@/components/proposal/ValidationTab.vue'
import { useProposalData } from '@/composables/useProposalData'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import type { LineQuote } from '@/lib/diffDiscussion'
import { nextStep } from '@/lib/nextstep'
import { fixesForBlocker, fixesForConflict } from '@/lib/fixes'
import type { FixId } from '@/lib/fixes'
import { parseIdentity, samePrincipal } from '@/lib/identity'
import { shortId, plural } from '@/lib/format'
import { requirementText } from '@/lib/roles'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import CopyButton from '@/ui/CopyButton.vue'
import Dialog from '@/ui/Dialog.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'
import Tabs from '@/ui/Tabs.vue'
import type { TabItem } from '@/ui/types'
import { conflictLabel, isDirectoryRenameConflict } from '@/lib/conflict-label'

const props = defineProps<{ id: string }>()
const { repo } = useRepoName()
const idRef = computed(() => props.id)
const auth = useAuth()
const route = useRoute(),
  router = useRouter()
const board = useBoard()
const toasts = useToasts()

const data = useProposalData(repo, idRef)
const { proposal, states } = data
usePageTitle(() => (proposal.value ? `${proposal.value.title} · ${repo.value}` : 'Proposal'))

type TabId = 'changes' | 'reviews' | 'validation' | 'provenance' | 'timeline'
const tab = computed<TabId>({
  get: () =>
    ['changes', 'reviews', 'validation', 'provenance', 'timeline'].includes(String(route.query.tab))
      ? (route.query.tab as TabId)
      : 'changes',
  set: (value) => {
    void router.replace({ query: { ...route.query, tab: value === 'changes' ? undefined : value } })
  },
})
const lineQuote = ref<LineQuote | null>(null)
const generalQuote = ref<LineQuote | null>(null)
const generalQuoteText = ref('')
const threadsSupported = computed(() => !!auth.capabilities?.features.includes('proposals.threads'))
watch(
  [repo, idRef, () => auth.token],
  () => {
    lineQuote.value = null
    generalQuote.value = null
    generalQuoteText.value = ''
  },
  { flush: 'sync' },
)
function discussLine(quote: LineQuote) {
  if (threadsSupported.value) lineQuote.value = quote
  else {
    generalQuoteText.value = ''
    generalQuote.value = quote
  }
  tab.value = 'reviews'
}
function useGeneralDiscussion(quote: LineQuote, body: string) {
  generalQuoteText.value = body
  generalQuote.value = quote
}
const tabs = computed<TabItem<TabId>[]>(() => [
  {
    id: 'changes',
    label: 'Files changed',
    icon: 'diff',
    count: data.diff.data.value?.files.length,
  },
  {
    id: 'reviews',
    label: 'Reviews',
    icon: 'comment',
    count: data.reviews.value.length || undefined,
  },
  {
    id: 'validation',
    label: 'Checks',
    icon: 'shield',
    count: data.prep.data.value?.validations.length || undefined,
  },
  {
    id: 'provenance',
    label: 'Where it came from',
    icon: 'sparkle',
    count: data.provenance.data.value?.length || undefined,
  },
  { id: 'timeline', label: 'Timeline', icon: 'history' },
])

const isOpen = computed(() => proposal.value?.state === 'open')
const authorIsAgent = computed(() =>
  proposal.value ? parseIdentity(proposal.value.author).kind === 'agent' : false,
)
const author = computed(() => (proposal.value ? parseIdentity(proposal.value.author) : null))
const prov = computed(() => data.provenance.data.value?.[0]?.record)
const prepData = computed(() => data.prep.data.value ?? null)
const blockers = computed(() => (isOpen.value ? (prepData.value?.blockers ?? []) : []))
const task = computed(() =>
  proposal.value?.task_id ? board.taskById.get(proposal.value.task_id) : undefined,
)

// ---- what the caller may do (hints; the server decides) ---------------------------------------------
const can = (a: Parameters<typeof auth.can>[1]) => auth.can(repo.value, a)

const isAuthor = computed(() =>
  proposal.value ? samePrincipal(proposal.value.author, auth.identity) : false,
)
const fixCtx = computed(() => ({ may: can, isAuthor: isAuthor.value }))
const planFor = (code: string) => fixesForBlocker(code, fixCtx.value)
const next = computed(() =>
  isOpen.value && prepData.value
    ? nextStep(prepData.value.blockers, prepData.value.conflicts, fixCtx.value)
    : '',
)
const conflictPlan = computed(() => fixesForConflict(fixCtx.value))

// Why an action is off, in words on the page (a tooltip alone is invisible on a phone and to the keyboard).
const unavailable = computed(() => {
  const out: string[] = []
  if (!can('integrate')) out.push(`Land: ${requirementText('integrate').toLowerCase()}.`)
  if (!can('review')) out.push(`Review: ${requirementText('review').toLowerCase()}.`)
  if (!can('requestValidation'))
    out.push(`Run the checks: ${requirementText('requestValidation').toLowerCase()}.`)
  return out
})

// ---- actions ----------------------------------------------------------------------------------------
const reviewing = ref(false)
const integrating = ref(false)
const closing = ref(false)
const busy = ref<'' | 'recheck' | 'validate' | 'enqueue' | 'close'>('')

async function recheck() {
  busy.value = 'recheck'
  try {
    await data.prep.reload()
    await data.attestations.reload()
    toasts.info(
      'Checked again',
      'Mergeability and policy were recomputed against the current stream.',
    )
  } finally {
    busy.value = ''
  }
}

async function requestValidation() {
  busy.value = 'validate'
  try {
    const jobs = await api.integration.validate(repo.value, props.id)
    if (jobs.length === 0) {
      toasts.warn(
        'No validation jobs were queued',
        'Either every required check already passes, or the policy gives no runner for them. A trusted runner can also submit an attestation by itself.',
      )
    } else {
      toasts.ok(
        `Queued ${plural(jobs.length, 'validation job')}`,
        jobs
          .map((j) => j.validation?.check ?? '')
          .filter(Boolean)
          .join(', '),
      )
    }
    tab.value = 'validation'
    await data.prep.reload()
  } catch (e) {
    toasts.error('Could not request validation', e)
  } finally {
    busy.value = ''
  }
}

async function enqueue() {
  busy.value = 'enqueue'
  try {
    await api.integration.enqueue(repo.value, props.id)
    toasts.ok('Added to the integration queue')
    board.touched(['queue'])
  } catch (e) {
    toasts.error('Could not enqueue', e)
  } finally {
    busy.value = ''
  }
}

async function closeProposal() {
  busy.value = 'close'
  try {
    await api.proposals.close(repo.value, props.id)
    toasts.ok('Proposal closed')
    closing.value = false
    board.touched(['proposals', 'queue'])
    data.refresh()
  } catch (e) {
    toasts.error('Could not close the proposal', e)
  } finally {
    busy.value = ''
  }
}

async function copyLink() {
  const url = window.location.origin + window.location.pathname
  try {
    await navigator.clipboard.writeText(url)
    toasts.ok('Link copied', 'Send it to someone who can review this change request.')
  } catch {
    toasts.warn('Could not copy', url)
  }
}

function doFix(id: FixId) {
  switch (id) {
    case 'validate':
      void requestValidation()
      break
    case 'review':
      reviewing.value = true
      break
    case 'land':
      integrating.value = true
      break
    case 'copy-link':
      void copyLink()
      break
    case 'checks':
      tab.value = 'validation'
      break
    case 'reviews':
      tab.value = 'reviews'
      break
    case 'provenance':
      tab.value = 'provenance'
      break
    case 'conflict':
      resolving.value = true
      break
  }
}

// A link such as `…/proposals/prop_…?do=review` (from the home page) opens what it names, once, when allowed.
const resolving = ref(false)
let didDo = ''
function runDo() {
  const want = typeof route.query.do === 'string' ? route.query.do : ''
  if (!want || didDo === want || !proposal.value || !isOpen.value) return
  const ok: FixId[] = ['validate', 'review', 'land', 'checks', 'reviews', 'provenance', 'conflict']
  if (!ok.includes(want as FixId)) return
  if (want === 'review' && !(can('review') && !isAuthor.value)) return
  if (want === 'land' && !can('integrate')) return
  if (want === 'validate') return // never start work just by opening a link
  didDo = want
  doFix(want as FixId)
}
watch(() => [proposal.value?.id, isOpen.value, route.query.do], runDo)
onMounted(runDo)

function changed() {
  board.touched(['proposals', 'streams', 'queue', 'predict'])
  data.refresh()
}
</script>

<template>
  <div class="proposal">
    <div v-if="data.detail.error.value && !proposal">
      <ErrorState
        what="this change request"
        :error="data.detail.error.value"
        @retry="data.detail.reload()"
      >
        <RouterLink :to="`/repos/${repo}/proposals`">All change requests</RouterLink>
      </ErrorState>
    </div>
    <div v-else-if="!proposal" class="loading" aria-busy="true">
      <Skeleton :height="30" width="55%" />
      <Skeleton :height="18" width="35%" />
      <Skeleton :height="120" radius="10px" />
    </div>

    <template v-else>
      <header class="head">
        <div class="crumb-row">
          <p class="crumb">
            <RouterLink :to="`/repos/${repo}/proposals`">Change requests</RouterLink>
            <span class="faint">/</span>
            <code class="mono">{{ shortId(proposal.id, 12) }}</code>
          </p>
          <details class="revision-details">
            <summary aria-label="Revision details">Details</summary>
            <ul class="revision-meta" role="list">
              <li v-if="author?.run" class="faint">
                run <code class="mono">{{ author.run }}</code>
              </li>
              <li v-if="prov?.model" class="faint">
                model <code class="mono">{{ prov.model }}</code>
              </li>
              <li class="faint">
                head <code class="mono">{{ shortId(proposal.head, 10) }}</code
                ><CopyButton :text="proposal.head" label="Copy head id" />
              </li>
              <li v-if="task" class="faint">
                task <strong>{{ task.title }}</strong>
              </li>
            </ul>
          </details>
        </div>
        <div class="title-row">
          <h1 data-testid="proposal-title">
            {{ proposal.title || '(untitled change request)' }}
          </h1>
          <Badge
            :tone="
              proposal.state === 'integrated'
                ? 'ok'
                : proposal.state === 'closed'
                  ? 'neutral'
                  : 'info'
            "
            data-testid="proposal-state"
            >{{ proposal.state === 'integrated' ? 'landed' : proposal.state }}</Badge
          >
          <div v-if="isOpen" class="primary-actions" role="group" aria-label="Primary actions">
            <Button
              :variant="can('review') ? 'primary' : 'secondary'"
              icon="check"
              :disabled="!can('review')"
              :disabled-reason="requirementText('review')"
              data-testid="act-review"
              @click="reviewing = true"
              >Review…</Button
            >
            <Button
              :variant="!can('review') && can('integrate') ? 'primary' : 'secondary'"
              icon="merge"
              :disabled="!can('integrate')"
              :disabled-reason="requirementText('integrate')"
              data-testid="act-integrate"
              @click="integrating = true"
              >Land…</Button
            >
          </div>
        </div>
        <div class="header-context">
          <ul class="meta" role="list">
            <li>
              <IdentityLabel :identity="proposal.author" :size="22" show-kind />
            </li>
            <li class="faint">opened <RelativeTime :ms="proposal.created_at_ms" /></li>
            <li>
              <Badge mono icon="branch" size="sm">{{ proposal.stream }}</Badge>
            </li>
          </ul>
        </div>
        <p v-if="proposal.description" class="desc">{{ proposal.description }}</p>
      </header>

      <Alert v-if="proposal.state === 'integrated'" tone="ok" title="Landed">
        This change is in <strong class="mono">{{ proposal.stream }}</strong>
        <template v-if="proposal.integrated_as">
          as <code class="mono">{{ shortId(proposal.integrated_as, 12) }}</code></template
        >.
        <RouterLink
          :to="{
            path: `/repos/${repo}/history`,
            query: { rev: proposal.integrated_as || proposal.head },
          }"
          >See it in history</RouterLink
        >
      </Alert>
      <Alert v-else-if="proposal.state === 'closed'" tone="info" title="Closed"
        >This change request was closed without landing.</Alert
      >

      <section v-if="states" class="readiness" aria-label="Merge readiness">
        <div class="readiness-row">
          <StateStrip :states="states" compact />
          <Button
            v-if="isOpen"
            size="sm"
            variant="ghost"
            class="recheck"
            icon="refresh"
            :loading="busy === 'recheck'"
            @click="recheck"
            >Check again</Button
          >
        </div>
        <p
          v-if="isOpen && prepData && !blockers.length && !prepData.conflicts.length"
          class="clear"
        >
          Nothing is in the way. This change request can land now.
        </p>
        <p
          v-if="next && (blockers.length || prepData?.conflicts.length || !can('integrate'))"
          class="next"
          :class="{ 'after-clear': !blockers.length && !prepData?.conflicts.length }"
          data-testid="next-step"
        >
          <strong>Next:</strong> {{ next }}
          <a v-if="blockers.length || prepData?.conflicts.length" href="#review-attention"
            >Details</a
          >
        </p>
      </section>

      <div class="review-layout">
        <div class="review-content">
          <Tabs v-model="tab" :tabs="tabs" label="Change request details">
            <template #changes>
              <ChangesTab
                :diff="data.diff.data.value"
                :review-scope="JSON.stringify([auth.identity, repo, proposal.id])"
                :can-discuss="
                  auth.can(repo, 'commentProposal') &&
                  !!auth.capabilities?.features.includes('proposals.comments')
                "
                :loading="data.diff.loading.value"
                :error="data.diff.error.value"
                @discuss="discussLine"
                @retry="data.diff.reload()"
              />
            </template>
            <template #reviews>
              <ReviewsTab
                :proposal="proposal"
                :reviews="data.reviews.value"
                :summary="data.reviewSummary.value"
                :can-review="can('review')"
                :review-reason="requirementText('review')"
                @review="reviewing = true"
              >
                <template #comment>
                  <ReviewThreads
                    v-if="threadsSupported"
                    :repo="repo"
                    :proposal="proposal"
                    :quote="lineQuote"
                    @quoted="lineQuote = null"
                    @general="useGeneralDiscussion"
                    @applied="changed" />
                  <ProposalComment
                    :repo="repo"
                    :proposal="proposal"
                    :quote="generalQuote"
                    :quote-text="generalQuoteText"
                    @quoted="generalQuote = null"
                    @done="changed"
                /></template>
              </ReviewsTab>
            </template>
            <template #validation>
              <ValidationTab
                :prep="prepData"
                :attestations="data.attestations.data.value ?? []"
                :can-request="can('requestValidation')"
                :request-reason="requirementText('requestValidation')"
                :requesting="busy === 'validate'"
                :open="isOpen"
                @request="requestValidation"
              />
            </template>
            <template #provenance>
              <ProvenanceTab
                :entries="data.provenance.data.value ?? []"
                :author-is-agent="authorIsAgent"
              />
            </template>
            <template #timeline>
              <TimelineTab :repo="repo" :proposal="proposal" />
            </template>
          </Tabs>
        </div>
        <aside class="review-sidebar" aria-label="Review context">
          <RequestedReviewers
            :repo="repo"
            :proposal="proposal"
            :reviewers="data.requestedReviewers.value"
            :reviews="data.reviews.value"
            @saved="changed"
          />

          <details
            v-if="
              isOpen && (blockers.length || prepData?.conflicts.length || data.prep.error.value)
            "
            id="review-attention"
            open
            class="rail-section policy-details"
          >
            <summary>What needs attention</summary>
            <div
              v-if="isOpen && prepData && prepData.conflicts.length"
              class="conflicts"
              data-testid="conflicts"
            >
              <h3>Conflicting paths</h3>
              <ul role="list">
                <li v-for="c in prepData.conflicts" :key="c.path">
                  <code class="mono">{{ c.path }}</code>
                  <Badge size="sm" tone="bad">{{ conflictLabel(c) }}</Badge>
                </li>
              </ul>
              <p
                v-if="prepData.conflicts.some(isDirectoryRenameConflict)"
                class="faint"
                data-testid="dir-rename-hint"
              >
                One side moved a whole directory while the other changed files under its old name;
                the merge follows such a move only when it is unambiguous and nothing is in the way.
              </p>
              <p class="faint">
                Nothing is guessed: someone has to choose, file by file, what the result should be.
              </p>
              <div class="fixrow">
                <Button
                  v-for="f in conflictPlan.fixes"
                  :key="f.id"
                  size="sm"
                  :variant="f.id === 'conflict' ? 'primary' : 'secondary'"
                  :data-testid="`fix-${f.id}`"
                  @click="doFix(f.id)"
                  >{{ f.label }}</Button
                >
              </div>
            </div>
            <div v-if="isOpen && blockers.length" class="blockers" data-testid="blockers">
              <h3>What is in the way</h3>
              <BlockerList :blockers="blockers" hints :plan="planFor" @fix="(id) => doFix(id)" />
            </div>
            <p v-if="isOpen && data.prep.error.value" class="faint">
              Could not recompute: {{ (data.prep.error.value as Error).message }}
            </p>
          </details>
          <section v-if="isOpen" class="rail-section secondary-actions" aria-label="More actions">
            <h2>More actions</h2>
            <Button
              v-if="
                can('commentProposal') && auth.capabilities?.features.includes('proposals.comments')
              "
              variant="ghost"
              icon="comment"
              @click="tab = 'reviews'"
              >Discuss…</Button
            >
            <Button
              variant="ghost"
              icon="play"
              :loading="busy === 'validate'"
              :disabled="!can('requestValidation')"
              :disabled-reason="requirementText('requestValidation')"
              data-testid="act-validate"
              @click="requestValidation"
              >Run the checks</Button
            >
            <Button
              variant="ghost"
              icon="stack"
              :loading="busy === 'enqueue'"
              :disabled="!can('enqueue')"
              :disabled-reason="requirementText('enqueue')"
              data-testid="act-enqueue"
              @click="enqueue"
              >Add to queue</Button
            >
            <Button
              variant="ghost"
              icon="x"
              :disabled="!can('closeProposal')"
              :disabled-reason="requirementText('closeProposal')"
              data-testid="act-close"
              @click="closing = true"
              >Close…</Button
            >
          </section>
          <details v-if="isOpen && unavailable.length" class="rail-section permissions">
            <summary>Your permissions</summary>
            <ul class="why" data-testid="unavailable">
              <li v-for="u in unavailable" :key="u">{{ u }}</li>
            </ul>
          </details>
        </aside>
      </div>

      <ConflictResolver
        v-if="resolving"
        v-model="resolving"
        :repo="repo"
        :proposal="proposal"
        @done="changed"
      />
      <ReviewDialog v-model="reviewing" :repo="repo" :proposal="proposal" @done="changed" />
      <IntegrateDialog
        v-model="integrating"
        :repo="repo"
        :proposal="proposal"
        :prep="prepData"
        :author-leases="data.authorLeases.value"
        :leases-error="data.leasesError.value"
        @done="changed"
      />
      <Dialog
        v-model="closing"
        title="Close this change request?"
        alert
        size="sm"
        :dismissable="busy !== 'close'"
        description="It will not land. Reviews and history are kept; you can open a new change request for the same change."
      >
        <template #footer>
          <Button :disabled="busy === 'close'" @click="closing = false">Keep it open</Button>
          <Button
            variant="danger"
            :loading="busy === 'close'"
            data-testid="confirm-close"
            @click="closeProposal"
            >Close change request</Button
          >
        </template>
      </Dialog>
    </template>
  </div>
</template>

<style scoped>
/* Hallmark · shape: app screen · theme: locked AgentGit · genre: modern-minimal · redesign: full · v2.0.0 */
/* Critique: code leads; one primary review action, quiet readiness strip and contextual side rail replace the control wall. */
.proposal {
  display: flex;
  flex-direction: column;
  gap: var(--s-4);
}
.loading {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.head {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.crumb-row {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  gap: var(--s-2);
  flex-wrap: wrap;
}
.revision-details[open] {
  flex-basis: 100%;
}
.crumb {
  font-size: var(--fs-sm);
  color: var(--text-2);
  display: flex;
  gap: var(--s-1);
  align-items: baseline;
}
.title-row {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
h1 {
  overflow-wrap: break-word;
  min-width: 0;
}
.primary-actions {
  margin-left: auto;
  display: flex;
  gap: var(--s-2);
}
.meta,
.revision-meta {
  list-style: none;
  margin: 0;
  padding: 0;
  font-size: var(--fs-sm);
}
.header-context {
  display: flex;
  align-items: center;
  gap: var(--s-4);
  flex-wrap: wrap;
}
.revision-details {
  color: var(--text-2);
  font-size: var(--fs-sm);
}
.revision-details summary {
  cursor: pointer;
}
.meta {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.meta li,
.revision-meta li {
  display: flex;
  align-items: center;
  gap: var(--s-1);
  min-width: 0;
}
.revision-meta {
  display: grid;
  gap: var(--s-3);
  margin-top: var(--s-3);
}
.revision-meta code {
  overflow-wrap: anywhere;
}
.desc {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--text-2);
  max-width: 90ch;
}
.readiness {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: var(--s-2) var(--s-3);
  padding: var(--s-3) 0;
  border-top: 1px solid var(--border);
  border-bottom: 1px solid var(--border);
}
.readiness-row {
  display: contents;
}
.readiness-row :deep(.strip) {
  grid-column: 1;
  grid-row: 1;
  align-self: center;
}
.recheck {
  grid-column: 2;
  grid-row: 1;
}
.clear,
.next {
  grid-column: 1 / -1;
  font-size: var(--fs-sm);
}
.clear {
  color: var(--text-2);
}
.next {
  color: var(--text);
}
.review-layout {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 252px;
  align-items: start;
  gap: var(--s-6);
}
.review-content,
.review-sidebar {
  min-width: 0;
}
.review-sidebar {
  display: grid;
  gap: var(--s-5);
  padding-top: var(--s-2);
}
.rail-section {
  border-top: 1px solid var(--border);
  padding-top: var(--s-4);
}
.rail-section summary,
.secondary-actions h2 {
  font-size: var(--fs-md);
  font-weight: 600;
}
.rail-section summary {
  cursor: pointer;
}
.secondary-actions {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: var(--s-1);
}
.secondary-actions h2 {
  margin-bottom: var(--s-2);
}
.secondary-actions :deep(.btn) {
  margin-left: calc(-1 * var(--s-3));
}
.why {
  list-style: none;
  margin: var(--s-3) 0 0;
  padding: 0;
  color: var(--text-2);
  font-size: var(--fs-sm);
  display: grid;
  gap: var(--s-2);
}
.conflicts,
.blockers {
  padding-top: var(--s-3);
}
h3 {
  font-size: var(--fs-sm);
  margin-bottom: var(--s-2);
}
.conflicts ul {
  list-style: none;
  margin: 0 0 var(--s-2);
  padding: 0;
  display: grid;
  gap: var(--s-2);
}
.conflicts li {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
}
.fixrow {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
  margin-top: var(--s-3);
}
@media (max-width: 1000px) {
  .review-layout {
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-5);
  }
  .review-sidebar {
    border-top: 1px solid var(--border);
    padding-top: var(--s-5);
  }
}
@media (max-width: 600px) {
  .primary-actions {
    width: 100%;
    margin-left: 0;
  }
  .primary-actions :deep(.btn) {
    flex: 1;
  }
  .readiness-row :deep(.strip) {
    grid-column: 1 / -1;
    grid-row: 1;
  }
  .recheck {
    grid-column: 2;
    grid-row: 2;
    align-self: center;
  }
  .clear,
  .next {
    grid-column: 1;
    grid-row: 2;
    align-self: center;
  }
  .next.after-clear {
    grid-column: 1 / -1;
    grid-row: 3;
  }
  .recheck :deep(.text) {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
}
</style>
