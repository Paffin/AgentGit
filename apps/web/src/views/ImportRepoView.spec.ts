import { enableAutoUnmount, flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'
import { ApiError } from '@/api/errors'
import { bundleCommands } from '@/api/migration'
import type { MigrationPlan } from '@/api/migration'
import type * as MigrationApi from '@/api/migration'
import ImportRepoView from './ImportRepoView.vue'

enableAutoUnmount(afterEach)
afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  if (clipboardDescriptor) Object.defineProperty(navigator, 'clipboard', clipboardDescriptor)
  else Reflect.deleteProperty(navigator, 'clipboard')
  if (execDescriptor) Object.defineProperty(document, 'execCommand', execDescriptor)
  else Reflect.deleteProperty(document, 'execCommand')
})
const mocks = vi.hoisted(() => ({ read: vi.fn(), prepare: vi.fn(), finish: vi.fn() }))
const auth = reactive({
  identity: 'user:alice',
  isAdminAnywhere: true,
  status: 'ready',
  token: 'alice-token',
})
vi.mock('@/stores/auth', () => ({ useAuth: () => auth }))
vi.mock('@/api/migration', async (importOriginal) => ({
  ...(await importOriginal<typeof MigrationApi>()),
  readMigration: mocks.read,
  prepareMigration: mocks.prepare,
  finishMigration: mocks.finish,
}))
const clipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
const execDescriptor = Object.getOwnPropertyDescriptor(document, 'execCommand')
const legacy = 'agentgit.migration'
const key = (identity: string) => `${legacy}:${encodeURIComponent(identity)}`
const plan = (name = 'acme/alice', id = 'saved-import'): MigrationPlan => ({
  id,
  name,
  source_sha256: 'digest',
  state: 'ready',
  report: {
    default_branch: 'main',
    commits: 1,
    blobs: 1,
    branches: [],
    tags: [],
    skipped_paths: [],
    skipped_submodules: 0,
    lossy_commits: 0,
    lfs_pointers: 0,
    other_refs: [],
  },
})
beforeEach(() => {
  sessionStorage.clear()
  auth.identity = 'user:alice'
  auth.isAdminAnywhere = true
  auth.status = 'ready'
  auth.token = 'alice-token'
  mocks.read.mockReset().mockResolvedValue(plan())
  mocks.prepare.mockReset().mockResolvedValue(plan())
  mocks.finish.mockReset().mockResolvedValue(plan())
})
async function show() {
  const w = mount(ImportRepoView, {
    global: { stubs: { RouterLink: { props: ['to'], template: '<a><slot/></a>' } } },
  })
  await flushPromises()
  return w
}
function button(w: ReturnType<typeof mount>, text: string) {
  return w.findAll('button').find((item) => item.text() === text)!
}

describe('resuming an import', () => {
  it('retains the legacy reference after a temporary failure and resumes the same import', async () => {
    sessionStorage.setItem(legacy, 'saved-import')
    mocks.read.mockRejectedValueOnce(new Error('Temporary outage'))
    const w = await show()
    expect(sessionStorage.getItem(legacy)).toBe('saved-import')
    expect(w.text()).toContain('Temporary outage')
    expect(button(w, 'Retry previous import')).toBeDefined()
    await button(w, 'Retry previous import').trigger('click')
    await flushPromises()
    expect(mocks.read).toHaveBeenNthCalledWith(2, 'saved-import', { token: 'alice-token' })
    expect(w.text()).toContain('Review before publishing')
    expect(sessionStorage.getItem(key(auth.identity))).toBe('saved-import')
    expect(sessionStorage.getItem(legacy)).toBeNull()
  })

  it.each([403, 404])(
    'keeps a %s reference until the user explicitly starts a new import',
    async (status) => {
      sessionStorage.setItem(key(auth.identity), 'unavailable-import')
      mocks.read.mockRejectedValueOnce(
        new ApiError({
          status,
          code: status === 404 ? 'not_found' : 'permission_denied',
          message: 'Import unavailable',
        }),
      )
      const w = await show()
      expect(w.text()).toContain('Import unavailable')
      expect(sessionStorage.getItem(key(auth.identity))).toBe('unavailable-import')
      await button(w, 'Start new import').trigger('click')
      expect(sessionStorage.getItem(key(auth.identity))).toBe('')
      expect(w.find('input[type=file]').exists()).toBe(true)
      expect(mocks.finish).not.toHaveBeenCalled()
    },
  )

  it('does not claim a legacy reference until its read succeeds', async () => {
    sessionStorage.setItem(legacy, 'saved-import')
    let finish!: (value: MigrationPlan) => void
    mocks.read.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show()
    expect(sessionStorage.getItem(key(auth.identity))).toBeNull()
    expect(sessionStorage.getItem(legacy)).toBe('saved-import')
    finish(plan())
    await flushPromises()
    expect(sessionStorage.getItem(key(auth.identity))).toBe('saved-import')
    w.unmount()
  })

  it('hides old account data and ignores its late response after an account switch', async () => {
    sessionStorage.setItem(key('user:alice'), 'alice-import')
    sessionStorage.setItem(key('user:bob'), 'bob-import')
    let finish!: (value: MigrationPlan) => void
    mocks.read
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = resolve
          }),
      )
      .mockResolvedValueOnce(plan('acme/bob', 'bob-import'))
    const w = await show()
    auth.identity = 'user:bob'
    await flushPromises()
    expect(w.text()).toContain('acme/bob')
    finish(plan('acme/alice', 'alice-import'))
    await flushPromises()
    expect(w.text()).not.toContain('acme/alice')
    expect(w.text()).toContain('acme/bob')
    expect(sessionStorage.getItem(key('user:alice'))).toBe('alice-import')
    expect(sessionStorage.getItem(key('user:bob'))).toBe('bob-import')
  })

  it('hides a pending report when the account loses its admin role', async () => {
    sessionStorage.setItem(key(auth.identity), 'saved-import')
    let finish!: (value: MigrationPlan) => void
    mocks.read.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show()
    auth.isAdminAnywhere = false
    await flushPromises()
    finish(plan())
    await flushPromises()
    expect(w.text()).toContain('An administrator needs to run the import')
    expect(w.text()).not.toContain('acme/alice')
    auth.isAdminAnywhere = true
    await flushPromises()
    expect(w.text()).toContain('acme/alice')
    expect(mocks.read).toHaveBeenCalledTimes(2)
  })

  it('explains unavailable browser storage without preventing a new import', async () => {
    vi.stubGlobal('sessionStorage', {
      getItem: () => {
        throw new Error('Storage blocked')
      },
    })
    const w = await show()
    expect(w.text()).toContain('Import recovery is unavailable')
    expect(w.find('input[type=file]').exists()).toBe(true)
    expect(mocks.read).not.toHaveBeenCalled()
  })
  it('requires signing in after 401 instead of offering an ineffective retry', async () => {
    sessionStorage.setItem(key(auth.identity), 'saved-import')
    mocks.read.mockRejectedValueOnce(
      new ApiError({ status: 401, code: 'unauthenticated', message: 'Credential expired' }),
    )
    const w = await show()
    expect(w.text()).toContain('Sign in again to continue')
    expect(button(w, 'Retry previous import')).toBeUndefined()
    const signIn = w
      .findAllComponents({ name: 'Button' })
      .find((item) => item.text() === 'Sign in again')!
    expect(signIn.props('to')).toEqual({ path: '/signin', query: { next: '/import' } })
    expect(sessionStorage.getItem(key(auth.identity))).toBe('saved-import')
  })

  it('waits for a verified account before resuming', async () => {
    auth.status = 'checking'
    sessionStorage.setItem(key(auth.identity), 'saved-import')
    const w = await show()
    expect(mocks.read).not.toHaveBeenCalled()
    auth.status = 'ready'
    await flushPromises()
    expect(mocks.read).toHaveBeenCalledWith('saved-import', { token: 'alice-token' })
    expect(w.text()).toContain('Review before publishing')
  })

  it('saves a late prepared id only for its original account without exposing the report', async () => {
    let finish!: (value: MigrationPlan) => void
    mocks.prepare.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show()
    await w.get('input[placeholder="team/project"]').setValue('acme/alice')
    const input = w.get('input[type=file]')
    Object.defineProperty(input.element, 'files', {
      value: [new File(['bundle'], 'source.bundle')],
      configurable: true,
    })
    await input.trigger('change')
    await w.get('form').trigger('submit')
    auth.identity = 'user:bob'
    auth.token = 'bob-token'
    await flushPromises()
    finish(plan('acme/alice', 'alice-late'))
    await flushPromises()
    expect(w.text()).not.toContain('acme/alice')
    expect(sessionStorage.getItem(key('user:alice'))).toBe('alice-late')
    expect(sessionStorage.getItem(key('user:bob'))).toBeNull()
    expect(mocks.prepare.mock.calls[0]![3]).toEqual({ token: 'alice-token' })
  })

  it('does not replace another account report with a late publish response', async () => {
    sessionStorage.setItem(key(auth.identity), 'saved-import')
    sessionStorage.setItem(key('user:bob'), 'bob-import')
    let finish!: (value: MigrationPlan) => void
    mocks.finish.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const w = await show()
    await button(w, 'Publish repository').trigger('click')
    mocks.read.mockResolvedValueOnce(plan('acme/bob', 'bob-import'))
    auth.identity = 'user:bob'
    auth.token = 'bob-token'
    await flushPromises()
    finish({ ...plan(), state: 'imported' })
    await flushPromises()
    expect(w.text()).toContain('acme/bob')
    expect(w.text()).not.toContain('Your repository is ready')
    expect(mocks.finish).toHaveBeenCalledWith('saved-import', false, { token: 'alice-token' })
  })

  it('keeps the report and legacy reference when saving the scoped reference fails', async () => {
    sessionStorage.setItem(legacy, 'saved-import')
    const storage = sessionStorage
    vi.stubGlobal('sessionStorage', {
      getItem: storage.getItem.bind(storage),
      setItem: () => {
        throw new Error('Storage blocked')
      },
      removeItem: storage.removeItem.bind(storage),
    })
    const w = await show()
    expect(w.text()).toContain('Import recovery is unavailable')
    expect(w.text()).toContain('Review before publishing')
    expect(sessionStorage.getItem(legacy)).toBe('saved-import')
  })
})

describe('copying migration commands', () => {
  it('copies through the shared fallback when the async clipboard is unavailable', async () => {
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true })
    let copied = ''
    Object.defineProperty(document, 'execCommand', {
      configurable: true,
      value: () => {
        copied = document.querySelector('textarea')?.value ?? ''
        return true
      },
    })
    const w = await show()
    await w
      .get('input[placeholder="https://github.com/team/project.git"]')
      .setValue('https://gitlab.example/team/first.git')
    await button(w, 'Copy commands').trigger('click')
    await flushPromises()
    expect(copied).toBe(bundleCommands('https://gitlab.example/team/first.git'))
    expect(button(w, 'Copied')).toBeDefined()
    expect(w.text()).not.toContain('Clipboard access is unavailable')
  })

  it('does not label changed instructions as copied after an older request completes', async () => {
    let finish!: () => void
    const writeText = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          finish = resolve
        }),
    )
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    const w = await show()
    const input = w.get('input[placeholder="https://github.com/team/project.git"]')
    await input.setValue('https://gitlab.example/team/first.git')
    await button(w, 'Copy commands').trigger('click')
    await input.setValue('https://gitlab.example/team/second.git')
    finish()
    await flushPromises()
    expect(writeText).toHaveBeenCalledWith(bundleCommands('https://gitlab.example/team/first.git'))
    expect(button(w, 'Copy commands')).toBeDefined()
    expect(button(w, 'Copied')).toBeUndefined()
  })
})

describe('copied SSH migration source', () => {
  it('does not suggest an invalid destination when a namespace normalizes to empty', async () => {
    const w = await show()
    await w
      .get('input[placeholder="https://github.com/team/project.git"]')
      .setValue('git@gitlab.com:---/project.git')
    expect(w.findAll('button').some((button) => button.text().startsWith('Use '))).toBe(false)
  })

  it('offers a valid AgentGit name for a nested GitLab namespace', async () => {
    const w = await show()
    await w
      .get('input[placeholder="https://github.com/team/project.git"]')
      .setValue('git@gitlab.com:group/subgroup/project.git')
    expect(w.text()).not.toContain('Use an HTTPS or SSH repository URL')
    expect(w.get('pre').text()).toContain("'git@gitlab.com:group/subgroup/project.git'")
    await button(w, 'Use subgroup/project').trigger('click')
    expect(w.get<HTMLInputElement>('input[placeholder="team/project"]').element.value).toBe(
      'subgroup/project',
    )
  })
})
