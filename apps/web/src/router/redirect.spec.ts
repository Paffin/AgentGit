// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { safeNext } from './redirect'

describe('post-sign-in destination', () => {
  it.each(['/', '/repos/acme/demo', '/repos/acme/demo/files?rev=main&path=src'])(
    'preserves the local destination %s',
    (path) => expect(safeNext(path)).toBe(path),
  )
  it.each([
    '',
    'repos/demo',
    '//evil.example',
    '/\\evil.example',
    'https://evil.example',
    'javascript:alert(1)',
  ])('rejects the external or nonabsolute destination %s', (path) =>
    expect(safeNext(path)).toBe('/'),
  )
})
