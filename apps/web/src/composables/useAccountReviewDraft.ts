import type { Ref } from 'vue'
import { api } from '@/api/instance'
import { useAccountDraft } from './useAccountDraft'
type Decision = 'approve' | 'request_changes' | 'comment'
type AccountDraft = Awaited<ReturnType<typeof api.proposals.reviewDraft>>
type Content = { head: string; body: string; decision: Decision }
interface Options {
  repo: () => string
  proposal: () => string
  open: () => boolean
  local: { head: Ref<string>; body: Ref<string>; decision: Ref<Decision> }
}
function parseDecision(value: string): Decision {
  if (value === 'approve' || value === 'request_changes' || value === 'comment') return value
  throw new Error('The saved account draft has an invalid decision.')
}
/** Private account storage for one review. */
export function useAccountReviewDraft(options: Options) {
  const local = options.local
  return useAccountDraft<AccountDraft, Content>({
    ...options,
    capability: 'proposals.review_drafts',
    maxBytes: 64 * 1024,
    sizeError: 'Account review drafts are limited to 64 KiB. Keep a local copy of longer notes.',
    content: () => ({
      head: local.head.value,
      body: local.body.value,
      decision: local.decision.value,
    }),
    same: (draft) =>
      local.head.value === draft.head &&
      local.body.value === draft.body &&
      local.decision.value === draft.decision,
    empty: () => !local.body.value && local.decision.value === 'approve',
    validate: (draft) => {
      parseDecision(draft.decision)
    },
    apply: (draft) => {
      local.head.value = draft.head
      local.body.value = draft.body
      local.decision.value = parseDecision(draft.decision)
    },
    read: (request) => api.proposals.reviewDraft(options.repo(), options.proposal(), request),
    write: (content, request) =>
      api.proposals.saveReviewDraft(options.repo(), options.proposal(), content, request),
    remove: (version, request) =>
      api.proposals.clearReviewDraft(options.repo(), options.proposal(), version, request),
  })
}
