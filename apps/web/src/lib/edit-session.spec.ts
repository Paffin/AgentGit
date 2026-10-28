// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import type { Api } from '@/api/endpoints'
import { proposeChange } from './edit'

const input = () => ({ repo: 'acme/app', stream: 'main', changes: [], title: 'Original title' })
function fixture() {
  const raw = {
    workspaces: {
      create: vi.fn().mockResolvedValue({ head: 'base', base: 'base' }),
      modify: vi.fn().mockResolvedValue({ head: 'edited' }),
      remove: vi.fn().mockResolvedValue({}),
    },
    proposals: { create: vi.fn().mockResolvedValue({ id: 'prop_1' }) },
  }
  return { raw, api: raw as unknown as Api }
}

describe('edit request ownership', () => {
  it('does not start an operation that was already cancelled', async () => {
    const { raw, api } = fixture()
    const controller = new AbortController()
    controller.abort()
    await expect(
      proposeChange(api, { ...input(), request: { signal: controller.signal } }),
    ).rejects.toMatchObject({ name: 'AbortError' })
    expect(raw.workspaces.create).not.toHaveBeenCalled()
  })

  it('does not write after cancellation while creating the workspace; cleanup retains the old session', async () => {
    const { raw, api } = fixture()
    let finish!: (value: object) => void
    raw.workspaces.create.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const controller = new AbortController()
    const request = {
      token: 'original-token',
      quietUnauthenticated: true,
      signal: controller.signal,
    }
    const pending = proposeChange(api, { ...input(), request })
    controller.abort()
    finish({ head: 'base', base: 'base' })
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' })
    expect(raw.workspaces.modify).not.toHaveBeenCalled()
    expect(raw.proposals.create).not.toHaveBeenCalled()
    expect(raw.workspaces.remove).toHaveBeenCalledWith('acme/app', expect.any(String), {
      token: 'original-token',
      quietUnauthenticated: true,
      timeoutMs: 5000,
    })
  })

  it('does not publish after cancellation while modifying the workspace', async () => {
    const { raw, api } = fixture()
    let finish!: (value: object) => void
    raw.workspaces.modify.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    const controller = new AbortController()
    const pending = proposeChange(api, { ...input(), request: { signal: controller.signal } })
    await vi.waitFor(() => expect(raw.workspaces.modify).toHaveBeenCalledTimes(1))
    controller.abort()
    finish({ head: 'edited' })
    await expect(pending).rejects.toMatchObject({ name: 'AbortError' })
    expect(raw.proposals.create).not.toHaveBeenCalled()
    expect(raw.workspaces.remove).toHaveBeenCalledTimes(1)
  })

  it('uses the initiating session for every stage', async () => {
    const { raw, api } = fixture()
    const request = { token: 'original-token', quietUnauthenticated: true }
    await proposeChange(api, { ...input(), request })
    expect(raw.workspaces.create.mock.calls[0]![2]).toEqual(request)
    expect(raw.workspaces.modify.mock.calls[0]![3]).toEqual(request)
    expect(raw.proposals.create.mock.calls[0]![2]).toEqual(request)
    expect(raw.workspaces.remove.mock.calls[0]![2]).toEqual({ ...request, timeoutMs: 5000 })
  })
})
