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
