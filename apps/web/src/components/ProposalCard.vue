<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { Proposal } from '@/api/types'
import { summarizeReviews } from '@/lib/fourstate'
import type { FourStates } from '@/lib/fourstate'
import { shortId, plural } from '@/lib/format'
import type { PrepState } from '@/stores/board'
import type { ProposalDetail } from '@/api/types'
import BlockerList from '@/components/BlockerList.vue'
import StateStrip from '@/components/StateStrip.vue'
import Badge from '@/ui/Badge.vue'
import Icon from '@/ui/Icon.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{
  repo: string
  proposal: Proposal
  detail?: ProposalDetail
  prep?: PrepState
  states: FourStates
}>()

const to = computed(() => `/repos/${props.repo}/proposals/${encodeURIComponent(props.proposal.id)}`)
const blockers = computed(() =>
  props.proposal.state === 'open' ? (props.prep?.data?.blockers ?? []) : [],
)
const conflicts = computed(() => props.prep?.data?.conflicts ?? [])
const files = computed(() => props.prep?.data?.changed_files.length ?? 0)
const reviews = computed(() =>
  props.detail ? summarizeReviews(props.detail.reviews, props.proposal.head) : null,
)
</script>

<template>
  <article
    class="card"
    :class="{ checking: proposal.state === 'open' && !prep?.data }"
    :data-proposal="proposal.id"
    :aria-label="`Proposal ${proposal.title}`"
  >
    <header class="head">
      <h3 class="title">
        <RouterLink :to="to">{{ proposal.title || '(untitled proposal)' }}</RouterLink>
      </h3>
      <Badge
        v-if="proposal.state !== 'open'"
        size="sm"
        :tone="proposal.state === 'integrated' ? 'ok' : 'neutral'"
        >{{ proposal.state }}</Badge
      >
    </header>
    <p class="meta">
      <IdentityLabel :identity="proposal.author" :size="18" />
      <span class="faint">opened <RelativeTime :ms="proposal.created_at_ms" /></span>
      <span class="faint mono">{{ shortId(proposal.id, 6) }}</span>
      <Badge size="sm" mono icon="branch">{{ proposal.stream }}</Badge>
    </p>
    <StateStrip :states="states" compact />
    <div v-if="conflicts.length" class="conflicts">
      <p class="label">Conflicting paths</p>
      <ul role="list">
        <li v-for="c in conflicts.slice(0, 4)" :key="c.path" class="mono">
          {{ c.path }} <span class="faint">({{ c.kind }})</span>
        </li>
        <li v-if="conflicts.length > 4" class="faint">and {{ conflicts.length - 4 }} more</li>
      </ul>
    </div>
    <BlockerList v-if="blockers.length" :blockers="blockers" :max="3" />
    <p v-else-if="proposal.state === 'open' && prep?.data && !conflicts.length" class="ready">
      <Icon name="check" :size="14" /> Nothing blocks integration right now.
    </p>
    <footer class="foot faint">
      <span v-if="files"><Icon name="file" :size="13" /> {{ plural(files, 'file') }}</span>
      <span v-if="reviews && reviews.approvals.length"
        ><Icon name="check" :size="13" /> {{ plural(reviews.approvals.length, 'approval') }}</span
      >
      <span v-if="reviews && reviews.staleApprovals.length" class="stale">
        <Icon name="alert" :size="13" />
        {{ plural(reviews.staleApprovals.length, 'stale approval') }}
      </span>
      <span v-if="reviews && reviews.changesRequested.length"
        ><Icon name="alert" :size="13" /> changes requested</span
      >
    </footer>
  </article>
</template>

<style scoped>
.card.checking {
  /* About what a card with a couple of blockers needs, so the verdicts arrive without moving what is below. */
  min-height: 11.5rem;
}
.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-3) var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
  min-width: 0;
}
.head {
  display: flex;
  justify-content: space-between;
  align-items: flex-start;
  gap: var(--s-2);
}
.title {
  font-size: var(--fs-md);
  line-height: 1.3;
  overflow-wrap: anywhere;
}
.title a {
  color: var(--text);
}
.title a:hover {
  color: var(--accent-text);
}
.meta {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 4px var(--s-3);
  font-size: var(--fs-sm);
}
.label {
  font-size: var(--fs-xs);
  font-weight: 620;
  color: var(--bad);
  text-transform: uppercase;
  letter-spacing: 0.03em;
}
.conflicts ul {
  list-style: none;
  margin: 2px 0 0;
  padding: 0;
  font-size: var(--fs-sm);
}
.ready {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--ok);
  font-size: var(--fs-sm);
  font-weight: 560;
}
.foot {
  display: flex;
  flex-wrap: wrap;
  gap: var(--s-3);
  font-size: var(--fs-xs);
  min-height: 16px;
}
.foot span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.stale {
  color: var(--warn);
}
</style>
