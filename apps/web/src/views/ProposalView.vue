<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { api } from '@/api/instance'
import BlockerList from '@/components/BlockerList.vue'
import StateStrip from '@/components/StateStrip.vue'
import ChangesTab from '@/components/proposal/ChangesTab.vue'
import IntegrateDialog from '@/components/proposal/IntegrateDialog.vue'
import ProvenanceTab from '@/components/proposal/ProvenanceTab.vue'
import ReviewDialog from '@/components/proposal/ReviewDialog.vue'
import ReviewsTab from '@/components/proposal/ReviewsTab.vue'
import TimelineTab from '@/components/proposal/TimelineTab.vue'
import ValidationTab from '@/components/proposal/ValidationTab.vue'
import { useProposalData } from '@/composables/useProposalData'
import { usePageTitle } from '@/composables/usePageTitle'
import { useRepoName } from '@/composables/useRepoName'
import { parseIdentity } from '@/lib/identity'
import { shortId, plural } from '@/lib/format'
import { requirementText } from '@/lib/roles'
import { useAuth } from '@/stores/auth'
import { useBoard } from '@/stores/board'
import { useToasts } from '@/stores/toasts'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import Card from '@/ui/Card.vue'
import CopyButton from '@/ui/CopyButton.vue'
import Dialog from '@/ui/Dialog.vue'
import ErrorState from '@/ui/ErrorState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'
import Skeleton from '@/ui/Skeleton.vue'
import Tabs from '@/ui/Tabs.vue'
import type { TabItem } from '@/ui/types'

const props = defineProps<{ id: string }>()
const { repo } = useRepoName()
const idRef = computed(() => props.id)
const auth = useAuth()
const board = useBoard()
const toasts = useToasts()

const data = useProposalData(repo, idRef)
const { proposal, states } = data
usePageTitle(() => (proposal.value ? `${proposal.value.title} · ${repo.value}` : 'Proposal'))

type TabId = 'changes' | 'reviews' | 'validation' | 'provenance' | 'timeline'
const tab = ref<TabId>('changes')
const tabs = computed<TabItem<TabId>[]>(() => [
  { id: 'changes', label: 'Changes', icon: 'diff', count: data.diff.data.value?.files.length },
  {
    id: 'reviews',
    label: 'Reviews',
    icon: 'comment',
    count: data.reviews.value.length || undefined,
  },
  {
    id: 'validation',
    label: 'Validation',
    icon: 'shield',
    count: data.prep.data.value?.validations.length || undefined,
  },
  {
    id: 'provenance',
    label: 'Provenance',
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

function changed() {
  board.touched(['proposals', 'streams', 'queue', 'predict'])
  data.refresh()
}
</script>

<template>
  <div class="proposal">
    <div v-if="data.detail.error.value && !proposal">
      <ErrorState
        what="this proposal"
        :error="data.detail.error.value"
        @retry="data.detail.reload()"
      >
        <RouterLink :to="`/repos/${repo}/proposals`">All proposals</RouterLink>
      </ErrorState>
    </div>
    <div v-else-if="!proposal" class="loading" aria-busy="true">
      <Skeleton :height="30" width="55%" />
      <Skeleton :height="18" width="35%" />
      <Skeleton :height="120" radius="10px" />
    </div>

    <template v-else>
      <header class="head">
        <p class="crumb">
          <RouterLink :to="`/repos/${repo}/proposals`">Proposals</RouterLink>
          <span class="faint">/</span>
          <code class="mono">{{ shortId(proposal.id, 12) }}</code>
        </p>
        <div class="title-row">
          <h1 data-testid="proposal-title">
            {{ proposal.title || '(untitled proposal)' }}
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
            >{{ proposal.state }}</Badge
          >
        </div>
        <ul class="meta" role="list">
          <li>
            <IdentityLabel :identity="proposal.author" :size="22" show-kind />
          </li>
          <li v-if="author?.run" class="faint">
            run <code class="mono">{{ author.run }}</code>
          </li>
          <li v-if="prov?.model" class="faint">
            model <code class="mono">{{ prov.model }}</code>
          </li>
          <li class="faint">opened <RelativeTime :ms="proposal.created_at_ms" /></li>
          <li>
            <Badge mono icon="branch" size="sm">{{ proposal.stream }}</Badge>
          </li>
          <li class="faint">
            head <code class="mono">{{ shortId(proposal.head, 10) }}</code
            ><CopyButton :text="proposal.head" label="Copy head id" />
          </li>
          <li v-if="task" class="faint">
            task <strong>{{ task.title }}</strong>
          </li>
        </ul>
        <p v-if="proposal.description" class="desc">{{ proposal.description }}</p>
      </header>

      <Alert v-if="proposal.state === 'integrated'" tone="ok" title="Integrated">
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
        >This proposal was closed without being integrated.</Alert
      >

      <Card
        v-if="states"
        title="Where it stands"
        icon="shield"
        :subtitle="isOpen ? 'Each state is checked on its own' : undefined"
      >
        <template v-if="isOpen" #actions>
          <Button size="sm" icon="refresh" :loading="busy === 'recheck'" @click="recheck"
            >Check again</Button
          >
        </template>
        <div class="stand">
          <StateStrip :states="states" />
          <div
            v-if="isOpen && prepData && prepData.conflicts.length"
            class="conflicts"
            data-testid="conflicts"
          >
            <h3>Conflicting paths</h3>
            <ul role="list">
              <li v-for="c in prepData.conflicts" :key="c.path">
                <code class="mono">{{ c.path }}</code>
                <Badge size="sm" tone="bad">{{ c.message || c.kind.replace('_', ' ') }}</Badge>
              </li>
            </ul>
            <p class="faint">
              Nothing guessed: resolve these in the author’s workspace (sync with resolutions) and
              update the proposal.
            </p>
          </div>
          <div v-if="isOpen && blockers.length" class="blockers" data-testid="blockers">
            <h3>What is in the way</h3>
            <BlockerList :blockers="blockers" hints />
          </div>
          <p v-else-if="isOpen && prepData && !prepData.conflicts.length" class="clear">
            Nothing blocks integration right now.
          </p>
          <p v-if="isOpen && data.prep.error.value" class="faint">
            Could not recompute: {{ (data.prep.error.value as Error).message }}
          </p>
        </div>

        <div v-if="isOpen" class="actions" role="group" aria-label="Actions">
          <Button
            variant="primary"
            icon="merge"
            :disabled="!can('integrate')"
            :disabled-reason="requirementText('integrate')"
            data-testid="act-integrate"
            @click="integrating = true"
            >Integrate…</Button
          >
          <Button
            icon="check"
            :disabled="!can('review')"
            :disabled-reason="requirementText('review')"
            data-testid="act-review"
            @click="reviewing = true"
            >Review…</Button
          >
          <Button
            icon="play"
            :loading="busy === 'validate'"
            :disabled="!can('requestValidation')"
            :disabled-reason="requirementText('requestValidation')"
            data-testid="act-validate"
            @click="requestValidation"
            >Request validation</Button
          >
          <Button
            icon="stack"
            :loading="busy === 'enqueue'"
            :disabled="!can('enqueue')"
            :disabled-reason="requirementText('enqueue')"
            data-testid="act-enqueue"
            @click="enqueue"
            >Add to queue</Button
          >
          <span class="spacer" />
          <Button
            variant="danger"
            icon="x"
            :disabled="!can('closeProposal')"
            :disabled-reason="requirementText('closeProposal')"
            data-testid="act-close"
            @click="closing = true"
            >Close…</Button
          >
        </div>
      </Card>

      <Tabs v-model="tab" :tabs="tabs" label="Proposal details">
        <template #changes>
          <ChangesTab
            :diff="data.diff.data.value"
            :loading="data.diff.loading.value"
            :error="data.diff.error.value"
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
          />
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

      <ReviewDialog v-model="reviewing" :repo="repo" :proposal="proposal" @done="changed" />
      <IntegrateDialog
        v-model="integrating"
        :repo="repo"
        :proposal="proposal"
        :prep="prepData"
        :author-leases="data.authorLeases.value"
        @done="changed"
      />
      <Dialog
        v-model="closing"
        title="Close this proposal?"
        alert
        size="sm"
        :dismissable="busy !== 'close'"
        description="It will not be integrated. Reviews and history are kept; you can open a new proposal for the same change."
      >
        <template #footer>
          <Button :disabled="busy === 'close'" @click="closing = false">Keep it open</Button>
          <Button
            variant="danger"
            :loading="busy === 'close'"
            data-testid="confirm-close"
            @click="closeProposal"
            >Close proposal</Button
          >
        </template>
      </Dialog>
    </template>
  </div>
</template>

<style scoped>
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
.crumb {
  font-size: var(--fs-sm);
  color: var(--text-2);
}
.title-row {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
h1 {
  overflow-wrap: anywhere;
}
.meta {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--s-2) var(--s-4);
  font-size: var(--fs-sm);
}
.meta li {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.desc {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
  color: var(--text-2);
  max-width: 90ch;
}
.stand {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.conflicts,
.blockers {
  padding: var(--s-3);
  border-radius: var(--r-sm);
  border: 1px solid var(--border);
  background: var(--surface-2);
}
.conflicts {
  border-color: var(--bad-line);
  background: var(--bad-soft);
}
h3 {
  font-size: var(--fs-sm);
  margin-bottom: var(--s-2);
}
.conflicts ul {
  list-style: none;
  margin: 0 0 var(--s-2);
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.clear {
  color: var(--ok);
  font-weight: 560;
}
.actions {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-2);
  align-items: center;
  margin-top: var(--s-4);
  padding-top: var(--s-4);
  border-top: 1px solid var(--border);
}
.spacer {
  flex: 1;
}
</style>
