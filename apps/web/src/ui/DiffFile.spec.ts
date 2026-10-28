import { mount } from '@vue/test-utils'
import { describe, expect, it } from 'vitest'
import type { FileDiff } from '@/api/types'
import DiffFile from './DiffFile.vue'

function file(lines: number, prefix = 'line'): FileDiff {
  return {
    path: 'README.md',
    status: 'modified',
    binary: false,
    additions: lines,
    deletions: 0,
    patch: `@@ -0,0 +1,${lines} @@\n${Array.from({ length: lines }, (_, i) => `+${prefix} ${i}`).join('\n')}\n`,
  }
}

describe('diff replacement', () => {
  it('does not expand a new huge patch just because the previous patch was open', async () => {
    const w = mount(DiffFile, { props: { file: file(2) } })
    expect(w.findAll('.row')).toHaveLength(2)
    await w.setProps({ file: file(700) })
    expect(w.findAll('.row')).toHaveLength(0)
    expect(w.get('button.toggle').attributes('aria-expanded')).toBe('false')
    expect(w.text()).toContain('lines hidden')
    w.unmount()
  })

  it('shows a new small patch after the previous huge patch was collapsed', async () => {
    const w = mount(DiffFile, { props: { file: file(700) } })
    expect(w.findAll('.row')).toHaveLength(0)
    await w.setProps({ file: file(2) })
    expect(w.findAll('.row')).toHaveLength(2)
    expect(w.get('button.toggle').attributes('aria-expanded')).toBe('true')
    w.unmount()
  })

  it('preserves manual expansion for an equivalent patch, then collapses new huge contents', async () => {
    const large = file(700)
    const w = mount(DiffFile, { props: { file: large } })
    await w.get('button.toggle').trigger('click')
    expect(w.findAll('.row')).toHaveLength(700)
    await w.setProps({ file: { ...large } })
    expect(w.findAll('.row')).toHaveLength(700)
    await w.setProps({ file: file(700, 'updated') })
    expect(w.findAll('.row')).toHaveLength(0)
    w.unmount()
  })

  it('preserves manual collapse for an equivalent patch and honors the collapsed default', async () => {
    const small = file(2)
    const w = mount(DiffFile, { props: { file: small } })
    await w.get('button.toggle').trigger('click')
    await w.setProps({ file: { ...small } })
    expect(w.findAll('.row')).toHaveLength(0)
    await w.setProps({ file: file(2, 'new') })
    expect(w.findAll('.row')).toHaveLength(2)
    await w.setProps({ collapsed: true, file: { ...small, path: 'other.md' } })
    expect(w.findAll('.row')).toHaveLength(0)
    w.unmount()
  })
})

describe('diff line number width', () => {
  it('fits either revision across a digit boundary and resets for a smaller patch', async () => {
    const wide = { ...file(2), patch: '@@ -999999,2 +9,2 @@\n context\n-old\n+new\n' }
    const w = mount(DiffFile, { props: { file: wide } })
    expect(w.get('.diff').attributes('style')).toContain('--number-width: 7ch')
    expect(w.findAll('.no').map((cell) => cell.text())).toContain('1000000')
    await w.setProps({ file: { ...wide, patch: '@@ -9,2 +999999,2 @@\n context\n-old\n+new\n' } })
    expect(w.get('.diff').attributes('style')).toContain('--number-width: 7ch')
    await w.setProps({ file: file(2) })
    expect(w.get('.diff').attributes('style')).toContain('--number-width: 3ch')
    w.unmount()
  })
})

describe('line discussion context', () => {
  it('keeps old and new line numbers distinct when quoting a replacement', async () => {
    const patch = { ...file(1), patch: '@@ -8,1 +12,1 @@\n-old value\n+new value\n' }
    const w = mount(DiffFile, { props: { file: patch, discussLines: true } })
    await w.get('button[aria-label="Discuss old line 8 of README.md"]').trigger('click')
    await w.get('button[aria-label="Discuss new line 12 of README.md"]').trigger('click')
    expect(w.emitted('discuss')).toEqual([
      [{ path: 'README.md', side: 'old', line: 8, text: 'old value' }],
      [{ path: 'README.md', side: 'new', line: 12, text: 'new value' }],
    ])
    w.unmount()
  })
})
