// @vitest-environment node
import { expect, it } from 'vitest'
import { preloadDependencies } from '../scripts/preload-policy'

it('keeps entry-page preloads intact', () => {
  const dependencies = ['assets/app-a.js', 'assets/vue-b.js']
  expect(preloadDependencies('assets/app-a.js', dependencies, { hostType: 'html' })).toEqual(
    dependencies,
  )
})
it('keeps lazy CSS without preloading JavaScript that native imports fetch', () => {
  const dependencies = ['assets/page-a.js', 'assets/vue-b.js', 'assets/page-c.css']
  expect(preloadDependencies('assets/page-a.js', dependencies, { hostType: 'js' })).toEqual([
    'assets/page-c.css',
  ])
  expect(dependencies).toEqual(['assets/page-a.js', 'assets/vue-b.js', 'assets/page-c.css'])
})
it('does not add a JavaScript hint when the import entry is absent', () => {
  expect(preloadDependencies('assets/page-a.js', ['assets/vue-b.js'], { hostType: 'js' })).toEqual(
    [],
  )
})
