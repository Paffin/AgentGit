import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import type { ComputedRef } from 'vue'
import type { RequestOptions } from '@/api/client'
import { ApiError, describeError } from '@/api/errors'
import { useAuth } from '@/stores/auth'

type SessionOpts = Pick<RequestOptions, 'token' | 'signal' | 'quietUnauthenticated'>

interface StoredDraft {
  present: boolean
  version: number
  head: string
  body: string
}
interface Content {
  head: string
  body: string
}
interface Options<D extends StoredDraft, C extends Content> {
  repo: () => string
  proposal: () => string
  open: () => boolean
  capability: string
  maxBytes: number
  sizeError: string
  content: () => C
  same: (draft: D) => boolean
  empty: () => boolean
  apply: (draft: D) => void
  validate: (draft: D) => void
  read: (request: SessionOpts) => Promise<D>
  write: (content: C & { expected_version: number }, request: SessionOpts) => Promise<D>
  remove: (version: number, request: SessionOpts) => Promise<D>
}

class AccountDraftState<D extends StoredDraft, C extends Content> {
  readonly loading = ref(false)
  readonly saving = ref(false)
  readonly loaded = ref(false)
  readonly error = ref('')
  readonly restored = ref(false)
  readonly available = shallowRef<D | null>(null)
  readonly supported: ComputedRef<boolean>
  readonly version: ComputedRef<number | null>
  readonly canSave: ComputedRef<boolean>
  private remote = shallowRef<D | null>(null)
  private readonly conflicted = ref(false)
  private generation = 0
  private adoptedVersion: number | null = null
  private controller: AbortController | undefined

  constructor(
    private options: Options<D, C>,
    private auth: ReturnType<typeof useAuth>,
  ) {
    this.supported = computed(() => !!auth.capabilities?.features.includes(options.capability))
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
    return this.options.content()
  }
  private apply(draft: D) {
    this.options.validate(draft)
    this.options.apply(draft)
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
  private accept(draft: D) {
    if (draft.present) this.options.validate(draft)
    this.remote.value = draft
    this.loaded.value = true
    if (!draft.present) return
    if (this.options.same(draft)) {
      this.adoptedVersion = draft.version
      return
    }
    if (this.options.empty()) this.apply(draft)
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
      const draft = await this.options.read(request)
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
    if (new TextEncoder().encode(submitted.body).length > this.options.maxBytes) {
      this.error.value = this.options.sizeError
      return false
    }
    const { mine, request } = this.begin()
    this.saving.value = true
    this.error.value = ''
    try {
      const draft = await this.options.write(
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
    return this.remote.value?.present ? this.adoptedVersion : null
  }
  async clearAfterSubmit(expectedVersion: number | null = this.cleanupVersion()): Promise<string> {
    if (!this.supported.value || expectedVersion === null) return ''
    const { mine, request } = this.begin()
    try {
      const draft = await this.options.remove(expectedVersion, request)
      if (mine === this.generation) this.remote.value = draft
      return ''
    } catch (cause) {
      return mine === this.generation ? describeError(cause) : ''
    }
  }
}

/** Versioned account storage shared by review and discussion drafts. */
export function useAccountDraft<D extends StoredDraft, C extends Content>(options: Options<D, C>) {
  const auth = useAuth()
  const state = new AccountDraftState(options, auth)
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
