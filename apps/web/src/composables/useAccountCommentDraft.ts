import type { Ref } from 'vue'
import { api } from '@/api/instance'
import { useAccountDraft } from './useAccountDraft'
type AccountDraft = Awaited<ReturnType<typeof api.proposals.commentDraft>>
type Content = { head: string; body: string }
interface Options {
  repo: () => string
  proposal: () => string
  open: () => boolean
  local: { head: Ref<string>; body: Ref<string> }
}
/** Private account storage for one proposal comment. */
export function useAccountCommentDraft(options: Options) {
  const local = options.local
  return useAccountDraft<AccountDraft, Content>({
    ...options,
    capability: 'proposals.comment_drafts',
    maxBytes: 16 * 1024,
    sizeError:
      'Account discussion drafts are limited to 16 KiB. Keep a local copy of longer notes.',
    content: () => ({ head: local.head.value, body: local.body.value }),
    same: (draft) => local.head.value === draft.head && local.body.value === draft.body,
    empty: () => !local.body.value,
    validate: () => {},
    apply: (draft) => {
      local.head.value = draft.head
      local.body.value = draft.body
    },
    read: (request) => api.proposals.commentDraft(options.repo(), options.proposal(), request),
    write: (content, request) =>
      api.proposals.saveCommentDraft(options.repo(), options.proposal(), content, request),
    remove: (version, request) =>
      api.proposals.clearCommentDraft(options.repo(), options.proposal(), version, request),
  })
}
