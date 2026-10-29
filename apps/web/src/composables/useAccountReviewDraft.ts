import { computed, onScopeDispose, ref, watch } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import { api } from '@/api/instance'
import { ApiError, describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'

type Decision = 'approve' | 'request_changes' | 'comment'
type AccountDraft = Awaited<ReturnType<typeof api.proposals.reviewDraft>>
interface LocalDraft {
  head: Ref<string>
  decision: Ref<Decision>
  body: Ref<string>
}
interface Options {
  repo: () => string
  proposal: () => string
  open: () => boolean
  local: LocalDraft
}
const isDecision = (value: string): value is Decision =>
  ['approve', 'request_changes', 'comment'].includes(value)

class AccountReviewDraft {
  readonly loading = ref(false)
  readonly saving = ref(false)
  readonly loaded = ref(false)
  readonly error = ref('')
  readonly restored = ref(false)
  readonly available = ref<AccountDraft | null>(null)
  readonly supported: ComputedRef<boolean>
  readonly version: ComputedRef<number | null>
  readonly canSave: ComputedRef<boolean>
  private remote = ref<AccountDraft | null>(null)
  private readonly conflicted = ref(false)
  private generation = 0
  private adoptedVersion: number | null = null
  private controller: AbortController | undefined

  constructor(
    private options: Options,
    private auth: ReturnType<typeof useAuth>,
  ) {
    this.supported = computed(
      () => !!auth.capabilities?.features.includes('proposals.review_drafts'),
    )
    this.version = computed(() => this.remote.value?.version ?? null)
    this.canSave = computed(() => this.readyToSave())
  }
  private readyToSave() {
    return (
      this.supported.value &&
      this.loaded.value &&
      !this.loading.value &&
      !this.saving.value &&
      !this.available.value &&
      !this.conflicted.value &&
      this.auth.status === 'ready'
    )
  }
  private content() {
    const local = this.options.local
    return { head: local.head.value, decision: local.decision.value, body: local.body.value }
  }
  private same(draft: AccountDraft) {
    const current = this.content()
    return (
      current.head === draft.head &&
      current.decision === draft.decision &&
      current.body === draft.body
    )
  }
  private apply(draft: AccountDraft) {
    if (!isDecision(draft.decision))
      throw new Error('The saved account draft has an invalid decision.')
    const local = this.options.local
    local.head.value = draft.head
    local.decision.value = draft.decision
    local.body.value = draft.body
    this.restored.value = true
    this.adoptedVersion = draft.version
  }
  cancelPending() {
    this.generation++
    this.controller?.abort()
    this.loading.value = this.saving.value = false
  }
  private begin() {
    const mine = ++this.generation
    this.controller?.abort()
    this.controller = new AbortController()
    return {
      mine,
      request: {
        token: this.auth.token,
        signal: this.controller.signal,
        quietUnauthenticated: true,
      },
    }
  }
  activate() {
    this.cancelPending()
    this.loaded.value = false
    this.remote.value = this.available.value = null
    this.error.value = ''
    this.restored.value = false
    this.conflicted.value = false
    this.adoptedVersion = null
    if (this.options.open()) void this.load()
  }
  private accept(draft: AccountDraft) {
    if (draft.present && !isDecision(draft.decision))
      throw new Error('The saved account draft has an invalid decision.')
    this.remote.value = draft
    this.loaded.value = true
    if (!draft.present) return
    if (this.same(draft)) {
      this.adoptedVersion = draft.version
      return
    }
    if (!this.options.local.body.value && this.options.local.decision.value === 'approve')
      this.apply(draft)
    else this.available.value = draft
  }
  async load() {
    if (
      !this.supported.value ||
      !this.options.open() ||
      this.auth.status !== 'ready' ||
      this.saving.value
    )
      return
    const { mine, request } = this.begin()
    this.loading.value = true
    this.loaded.value = false
    this.error.value = ''
    this.restored.value = false
    this.available.value = null
    this.adoptedVersion = null
    this.conflicted.value = false
    try {
      const draft = await api.proposals.reviewDraft(
        this.options.repo(),
        this.options.proposal(),
        request,
      )
      if (mine === this.generation) this.accept(draft)
    } catch (cause) {
      if (mine === this.generation) this.error.value = describeError(cause)
    } finally {
      if (mine === this.generation) this.loading.value = false
    }
  }
  useAccount() {
    if (!this.available.value) return
    this.apply(this.available.value)
    this.available.value = null
  }
  keepLocal() {
    this.available.value = null
    this.restored.value = false
    this.adoptedVersion = null
  }
  async save(): Promise<boolean> {
    if (!this.canSave.value) return false
    const submitted = this.content()
    if (new TextEncoder().encode(submitted.body).length > 64 * 1024) {
      this.error.value =
        'Account review drafts are limited to 64 KiB. Keep a local copy of longer notes.'
      return false
    }
    const { mine, request } = this.begin()
    this.saving.value = true
    this.error.value = ''
    try {
      const draft = await api.proposals.saveReviewDraft(
        this.options.repo(),
        this.options.proposal(),
        { ...submitted, expected_version: this.version.value! },
        request,
      )
      if (mine !== this.generation) return false
      this.remote.value = draft
      this.adoptedVersion = draft.version
      return true
    } catch (cause) {
      if (mine === this.generation) {
        this.error.value = describeError(cause)
        this.conflicted.value = cause instanceof ApiError && cause.code === 'failed_precondition'
      }
      return false
    } finally {
      if (mine === this.generation) this.saving.value = false
    }
  }
  cleanupVersion(): number | null {
    return this.remote.value?.present && this.adoptedVersion === this.version.value
      ? this.adoptedVersion
      : null
  }
  async clearAfterSubmit(expectedVersion: number | null = this.cleanupVersion()): Promise<string> {
    if (!this.supported.value || expectedVersion === null) return ''
    const { mine, request } = this.begin()
    try {
      const draft = await api.proposals.clearReviewDraft(
        this.options.repo(),
        this.options.proposal(),
        expectedVersion,
        request,
      )
      if (mine === this.generation) this.remote.value = draft
      return ''
    } catch (cause) {
      return mine === this.generation ? describeError(cause) : ''
    }
  }
}

/** Private account storage for one review, with explicit local/remote conflict choices. */
export function useAccountReviewDraft(options: Options) {
  const auth = useAuth()
  const state = new AccountReviewDraft(options, auth)
  watch(
    [
      options.repo,
      options.proposal,
      () => auth.identity,
      () => auth.token,
      () => auth.status,
      state.supported,
      options.open,
    ],
    () => state.activate(),
    { immediate: true, flush: 'sync' },
  )
  onScopeDispose(() => state.cancelPending())
  return {
    supported: state.supported,
    loading: state.loading,
    saving: state.saving,
    error: state.error,
    restored: state.restored,
    available: state.available,
    version: state.version,
    canSave: state.canSave,
    load: () => state.load(),
    useAccount: () => state.useAccount(),
    keepLocal: () => state.keepLocal(),
    save: () => state.save(),
    cleanupVersion: () => state.cleanupVersion(),
    clearAfterSubmit: (version?: number | null) => state.clearAfterSubmit(version),
    cancelPending: () => state.cancelPending(),
  }
}
