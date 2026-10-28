<script setup lang="ts">
import { computed } from 'vue'
import type { Proposal, Review } from '@/api/types'
import type { ReviewSummary } from '@/lib/fourstate'
import { shortId, plural } from '@/lib/format'
import Alert from '@/ui/Alert.vue'
import Badge from '@/ui/Badge.vue'
import Button from '@/ui/Button.vue'
import EmptyState from '@/ui/EmptyState.vue'
import IdentityLabel from '@/ui/IdentityLabel.vue'
import RelativeTime from '@/ui/RelativeTime.vue'

const props = defineProps<{
  proposal: Proposal
  reviews: readonly Review[]
  summary: ReviewSummary | null
  canReview: boolean
  reviewReason: string
}>()
defineEmits<{ review: [] }>()

const sorted = computed(() => [...props.reviews].sort((a, b) => b.id - a.id))
const DECISION = {
  approve: { label: 'Approved', tone: 'ok', icon: 'check' },
  request_changes: { label: 'Requested changes', tone: 'warn', icon: 'alert' },
  comment: { label: 'Commented', tone: 'neutral', icon: 'comment' },
} as const
type Tone = 'ok' | 'warn' | 'neutral'
const info = (d: string): { label: string; tone: Tone; icon: 'check' | 'alert' | 'comment' } =>
  d in DECISION
    ? DECISION[d as keyof typeof DECISION]
    : { label: d, tone: 'neutral', icon: 'comment' }
const stale = (r: Review) => r.head !== props.proposal.head
</script>

<template>
  <div class="reviews" data-testid="reviews-tab">
    <div class="bar">
      <p v-if="summary" class="muted">
        {{ plural(summary.approvals.length, 'approval') }} count for the current head<template
          v-if="summary.staleApprovals.length"
        >
          · {{ plural(summary.staleApprovals.length, 'approval') }} no longer
          {{ summary.staleApprovals.length === 1 ? 'counts' : 'count' }}</template
        >.
      </p>
      <Button
        variant="primary"
        icon="check"
        :disabled="!canReview || proposal.state !== 'open'"
        :disabled-reason="
          proposal.state !== 'open' ? 'Only open proposals can be reviewed' : reviewReason
        "
        @click="$emit('review')"
      >
        Write a review
      </Button>
    </div>
    <Alert
      v-if="summary && summary.staleApprovals.length"
      tone="warn"
      title="Some approvals are stale"
    >
      Approvals are bound to the head the reviewer looked at. The proposal moved to
      <code class="mono">{{ shortId(proposal.head, 8) }}</code
      >, so those reviewers need to look again.
    </Alert>
    <slot name="comment" />
    <EmptyState
      v-if="sorted.length === 0"
      icon="comment"
      title="No reviews yet"
      description="Reviews are bound to the exact head they looked at; if the head changes, earlier approvals stop counting."
      compact
    />
    <ol v-else class="list">
      <li
        v-for="r in sorted"
        :key="r.id"
        class="rev"
        :class="{ stale: stale(r) }"
        :data-review="r.id"
      >
        <div class="top">
          <IdentityLabel :identity="r.reviewer" />
          <Badge :tone="info(r.decision).tone" :icon="info(r.decision).icon" size="sm">{{
            info(r.decision).label
          }}</Badge>
          <Badge v-if="stale(r) && r.decision !== 'comment'" tone="warn" size="sm" icon="clock"
            >stale</Badge
          >
          <span class="faint when"><RelativeTime :ms="r.timestamp_ms" /></span>
        </div>
        <p v-if="r.body" class="body">{{ r.body }}</p>
        <p class="faint head">
          Looked at <code class="mono">{{ shortId(r.head, 8) }}</code>
          <template v-if="stale(r)">
            · the proposal is now at
            <code class="mono">{{ shortId(proposal.head, 8) }}</code
            >,
            {{
              r.decision === 'comment'
                ? 'this comment refers to the earlier head'
                : 'this review no longer counts'
            }}</template
          >
        </p>
      </li>
    </ol>
  </div>
</template>

<style scoped>
.reviews {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.bar {
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}
.rev {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  padding: var(--s-3) var(--s-4);
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}
.rev.stale {
  border-style: dashed;
  border-color: var(--warn-line);
}
.top {
  display: flex;
  align-items: center;
  gap: var(--s-3);
  flex-wrap: wrap;
}
.when {
  margin-left: auto;
  font-size: var(--fs-sm);
}
.body {
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.head {
  font-size: var(--fs-xs);
}
</style>
