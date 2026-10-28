import { mount } from '@vue/test-utils'
import { beforeEach, expect, it } from 'vitest'
import type { FileDiff } from '@/api/types'
import ChangesTab from './ChangesTab.vue'
const files: FileDiff[] = ['src/one.ts', 'docs/two.md'].map((path) => ({
  path,
  status: 'added',
  binary: false,
  additions: 1,
  deletions: 0,
  patch: '@@ -0,0 +1 @@\n+hello\n',
}))
const diff = { files, base: 'base-one', head: 'head-one', truncated: false }
beforeEach(() => sessionStorage.clear())
it('filters both navigation and patches without losing the server truncation warning', async () => {
  const w = mount(ChangesTab, {
    props: {
      diff: { ...diff, truncated: true },
      loading: false,
      error: null,
      reviewScope: 'reader:repo:proposal',
    },
  })
  await w.get('input[type="search"]').setValue('DOCS/')
  expect(w.findAll('section.file')).toHaveLength(1)
  expect(w.get('section.file').text()).toContain('docs/two.md')
  expect(w.get('nav').text()).not.toContain('src/one.ts')
  await w.get('input[type="search"]').setValue('nothing')
  expect(w.text()).toContain('No matching files')
  expect(w.text()).toContain('cut off by the server')
  w.unmount()
})
it('remembers viewed files for the exact account, proposal and comparison, never as approval', async () => {
  const props = { diff, loading: false, error: null, reviewScope: 'reader:repo:proposal' }
  let w = mount(ChangesTab, { props })
  await w.get('input[aria-label="Viewed src/one.ts"]').setValue(true)
  w.unmount()
  w = mount(ChangesTab, { props })
  expect((w.get('input[aria-label="Viewed src/one.ts"]').element as HTMLInputElement).checked).toBe(
    true,
  )
  await w.get('input[aria-label="Hide viewed files"]').setValue(true)
  expect(w.findAll('section.file')).toHaveLength(1)
  await w.setProps({ diff: { ...diff, head: 'head-two' } })
  expect(w.findAll('section.file')).toHaveLength(2)
  expect((w.get('input[aria-label="Viewed src/one.ts"]').element as HTMLInputElement).checked).toBe(
    false,
  )
  await w.get('input[aria-label="Viewed src/one.ts"]').setValue(true)
  await w.setProps({ reviewScope: 'other:repo:proposal' })
  expect((w.get('input[aria-label="Viewed src/one.ts"]').element as HTMLInputElement).checked).toBe(
    false,
  )
  w.unmount()
})
