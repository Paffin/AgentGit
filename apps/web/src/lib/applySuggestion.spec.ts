// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import type { Api } from '@/api/endpoints'
import { applySuggestion, previewSuggestion } from './applySuggestion'

const anchor = {
  head: 'seen',
  base: '',
  path: 'script.sh',
  side: 'new',
  line: 1,
  text: '\uFEFFold',
}
const context = { token: 'original-session', quietUnauthenticated: true }
function fixture() {
  const raw = {
    tree: {
      resolve: vi.fn().mockResolvedValue({ entry: { file_type: 'regular', executable: true } }),
      file: vi
        .fn()
        .mockResolvedValue({ bytes: new TextEncoder().encode('\uFEFFold\r\n'), truncated: false }),
    },
    workspaces: {
      create: vi.fn().mockResolvedValue({ head: 'seen' }),
      modify: vi.fn().mockResolvedValue({ head: 'replacement' }),
      remove: vi.fn().mockResolvedValue({}),
    },
    proposals: { update: vi.fn().mockResolvedValue({}) },
  }
  return { raw, api: raw as unknown as Api }
}

describe('suggestion publication', () => {
  it('preserves executable mode and BOM while replacing exact text', async () => {
    const { api, raw } = fixture()
    const preview = await previewSuggestion(api, 'team/repo', anchor, 'new', context)
    expect(preview.after).toBe('\uFEFFnew\r\n')
    expect(preview.change.executable).toBe(true)
    await applySuggestion(api, 'team/repo', 'proposal', 'dev', 'seen', preview, context)
    expect(raw.workspaces.create).toHaveBeenCalledWith(
      'team/repo',
      expect.objectContaining({ stream: 'dev', from_rev: 'seen' }),
      context,
    )
    expect(raw.proposals.update).toHaveBeenCalledWith(
      'team/repo',
      'proposal',
      'replacement',
      'seen',
      context,
    )
    expect(raw.workspaces.remove).toHaveBeenCalledWith(
      'team/repo',
      expect.any(String),
      expect.objectContaining({ token: 'original-session' }),
    )
  })

  it('cleans up the workspace when a concurrent proposal update is refused', async () => {
    const { api, raw } = fixture()
    const preview = await previewSuggestion(api, 'team/repo', anchor, 'new', context)
    raw.proposals.update.mockRejectedValue(new Error('head changed'))
    await expect(
      applySuggestion(api, 'team/repo', 'proposal', 'dev', 'seen', preview, context),
    ).rejects.toThrow('head changed')
    expect(raw.workspaces.remove).toHaveBeenCalledOnce()
  })

  it('does not update a proposal after the session is cancelled', async () => {
    const { api, raw } = fixture()
    const preview = await previewSuggestion(api, 'team/repo', anchor, 'new', context)
    const controller = new AbortController()
    raw.workspaces.modify.mockImplementation(async () => {
      controller.abort()
      return { head: 'replacement' }
    })
    await expect(
      applySuggestion(api, 'team/repo', 'proposal', 'dev', 'seen', preview, {
        ...context,
        signal: controller.signal,
      }),
    ).rejects.toMatchObject({ name: 'AbortError' })
    expect(raw.proposals.update).not.toHaveBeenCalled()
    expect(raw.workspaces.remove).toHaveBeenCalledOnce()
  })

  it('refuses symlinks and truncated files', async () => {
    const { api, raw } = fixture()
    raw.tree.resolve.mockResolvedValue({ entry: { file_type: 'symlink', executable: false } })
    await expect(previewSuggestion(api, 'team/repo', anchor, 'new', context)).rejects.toThrow(
      'regular text files',
    )
    raw.tree.resolve.mockResolvedValue({ entry: { file_type: 'regular', executable: false } })
    raw.tree.file.mockResolvedValue({ bytes: new Uint8Array(), truncated: true })
    await expect(previewSuggestion(api, 'team/repo', anchor, 'new', context)).rejects.toThrow(
      'regular text files',
    )
  })
})
