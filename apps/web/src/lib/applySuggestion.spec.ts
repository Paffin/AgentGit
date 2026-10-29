// @vitest-environment node
import { describe, expect, it, vi } from 'vitest'
import type { Api } from '@/api/endpoints'
import {
  applySuggestion,
  previewSuggestion,
  previewSuggestions,
  applySuggestions,
} from './applySuggestion'

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

describe('batched suggestion publication', () => {
  it('combines same-file suggestions against their original line numbers', async () => {
    const { api, raw } = fixture()
    raw.tree.file.mockResolvedValue({
      bytes: new TextEncoder().encode('\uFEFFold\r\ntwo\r\n'),
      truncated: false,
    })
    const previews = await previewSuggestions(
      api,
      'team/repo',
      'seen',
      [
        { anchor, replacement: 'new' },
        { anchor: { ...anchor, line: 2, text: 'two' }, replacement: 'more\nlines' },
      ],
      context,
    )
    expect(previews).toHaveLength(1)
    expect(previews[0]!.after).toBe('\uFEFFnew\r\nmore\r\nlines\r\n')
    expect(previews[0]!.change.executable).toBe(true)
    expect(raw.tree.file).toHaveBeenCalledOnce()
  })

  it('publishes multiple files as one workspace change and one head update', async () => {
    const { api, raw } = fixture()
    const previews = await previewSuggestions(
      api,
      'team/repo',
      'seen',
      [
        { anchor, replacement: 'new' },
        { anchor: { ...anchor, path: 'another.sh' }, replacement: 'another' },
      ],
      context,
    )
    await applySuggestions(api, 'team/repo', 'proposal', 'dev', 'seen', previews, context)
    expect(raw.workspaces.modify).toHaveBeenCalledOnce()
    const body = raw.workspaces.modify.mock.calls[0]![2]
    expect(body.expected_head).toBe('seen')
    expect(body.changes.map((change: { path: string }) => change.path)).toEqual([
      'another.sh',
      'script.sh',
    ])
    expect(raw.proposals.update).toHaveBeenCalledOnce()
    expect(raw.workspaces.remove).toHaveBeenCalledOnce()
  })

  it('rejects competing replacements for one original line before reading files', async () => {
    const { api, raw } = fixture()
    await expect(
      previewSuggestions(
        api,
        'team/repo',
        'seen',
        [
          { anchor, replacement: 'one' },
          { anchor, replacement: 'two' },
        ],
        context,
      ),
    ).rejects.toThrow(/same line/)
    expect(raw.tree.file).not.toHaveBeenCalled()
  })

  it('rejects mixed revisions and empty batches before reading files', async () => {
    const { api, raw } = fixture()
    await expect(
      previewSuggestions(
        api,
        'team/repo',
        'seen',
        [{ anchor: { ...anchor, head: 'stale' }, replacement: 'new' }],
        context,
      ),
    ).rejects.toThrow(/revision/)
    await expect(previewSuggestions(api, 'team/repo', 'seen', [], context)).rejects.toThrow(
      /Select/,
    )
    expect(raw.tree.file).not.toHaveBeenCalled()
  })
})

describe('suggestion limits and ordering', () => {
  it('applies lower original lines after higher lines even when they insert new lines', async () => {
    const { api, raw } = fixture()
    raw.tree.file.mockResolvedValue({
      bytes: new TextEncoder().encode('first\nsecond\n'),
      truncated: false,
    })
    const [preview] = await previewSuggestions(
      api,
      'team/repo',
      'seen',
      [
        { anchor: { ...anchor, text: 'first' }, replacement: 'one\nextra' },
        { anchor: { ...anchor, line: 2, text: 'second' }, replacement: 'two' },
      ],
      context,
    )
    expect(preview!.after).toBe('one\nextra\ntwo\n')
  })

  it('accepts exactly 100 suggestions and rejects 101 before reading files', async () => {
    const { api, raw } = fixture()
    const suggestions = Array.from({ length: 100 }, (_, index) => ({
      anchor: { ...anchor, path: `f${index}` },
      replacement: 'new',
    }))
    expect(await previewSuggestions(api, 'team/repo', 'seen', suggestions, context)).toHaveLength(
      100,
    )
    raw.tree.file.mockClear()
    await expect(
      previewSuggestions(api, 'team/repo', 'seen', [...suggestions, suggestions[0]!], context),
    ).rejects.toThrow(/100/)
    expect(raw.tree.file).not.toHaveBeenCalled()
  })

  it('accepts a 4 MiB result and rejects one additional byte', async () => {
    const { api, raw } = fixture()
    raw.tree.file.mockResolvedValue({ bytes: new TextEncoder().encode('old'), truncated: false })
    const replacement = 'x'.repeat(4 * 1024 * 1024)
    expect(
      await previewSuggestions(
        api,
        'team/repo',
        'seen',
        [{ anchor: { ...anchor, text: 'old' }, replacement }],
        context,
      ),
    ).toHaveLength(1)
    await expect(
      previewSuggestions(
        api,
        'team/repo',
        'seen',
        [{ anchor: { ...anchor, text: 'old' }, replacement: replacement + 'x' }],
        context,
      ),
    ).rejects.toThrow(/4 MiB/)
  })

  it('enforces file fetch bounds, malformed UTF-8 and missing entries', async () => {
    const { api, raw } = fixture()
    await previewSuggestion(api, 'team/repo', anchor, 'new', context)
    expect(raw.tree.file).toHaveBeenCalledWith(
      'team/repo',
      'seen',
      'script.sh',
      1024 * 1024,
      context,
    )
    raw.tree.file.mockResolvedValue({
      bytes: new Uint8Array([111, 108, 100, 10, 255]),
      truncated: false,
    })
    await expect(
      previewSuggestion(api, 'team/repo', { ...anchor, text: 'old' }, 'new', context),
    ).rejects.toThrow()
    raw.tree.resolve.mockResolvedValue({})
    await expect(previewSuggestion(api, 'team/repo', anchor, 'new', context)).rejects.toThrow(
      /regular text files/,
    )
  })

  it('bounds prepared files and records a useful change message', async () => {
    const { api, raw } = fixture()
    const preview = await previewSuggestion(api, 'team/repo', anchor, 'new', context)
    for (const count of [0, 101]) {
      await expect(
        applySuggestions(api, 'team/repo', 'p', 'dev', 'seen', Array(count).fill(preview), context),
      ).rejects.toThrow(/100/)
    }
    expect(raw.workspaces.create).not.toHaveBeenCalled()
    await applySuggestions(api, 'team/repo', 'p', 'dev', 'seen', Array(100).fill(preview), context)
    expect(raw.workspaces.modify.mock.calls[0]![2].message).toBe('Apply review suggestions')
    await applySuggestions(api, 'team/repo', 'p', 'dev', 'seen', [preview], context)
    expect(raw.workspaces.modify.mock.calls[1]![2].message).toBe(
      'Apply review suggestion to script.sh',
    )
    expect(raw.workspaces.remove).toHaveBeenCalledWith(
      'team/repo',
      expect.any(String),
      expect.objectContaining({ quietUnauthenticated: true }),
    )
  })
})

describe('range suggestion previews', () => {
  it('previews one range against the anchored revision', async () => {
    const { api, raw } = fixture()
    raw.tree.file.mockResolvedValue({
      bytes: new TextEncoder().encode('\uFEFFold\r\nsecond\r\nlast\r\n'),
      truncated: false,
    })
    const preview = await previewSuggestion(api, 'team/repo', anchor, 'new', context, 2)
    expect(preview.after).toBe('\uFEFFnew\r\nlast\r\n')
  })

  it('rejects overlapping ranges before reading or modifying any file', async () => {
    const { api, raw } = fixture()
    await expect(
      previewSuggestions(
        api,
        'team/repo',
        'seen',
        [
          { anchor, replacement: 'new', lines: 2 },
          { anchor: { ...anchor, line: 2, text: 'second' }, replacement: 'other' },
        ],
        context,
      ),
    ).rejects.toThrow(/overlap/)
    expect(raw.tree.file).not.toHaveBeenCalled()
    expect(raw.workspaces.create).not.toHaveBeenCalled()
  })
})
