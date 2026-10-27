// @vitest-environment node
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { glossaryTable } from '../src/lib/glossary'

// The words table in the guide is the glossary of the UI, printed. Regenerate it from `glossaryTable()` when this fails.
describe('docs/guides/web-ui.md "Plain words"', () => {
  it('contains the glossary exactly as the UI has it', () => {
    const guide = readFileSync(resolve(process.cwd(), '../../docs/guides/web-ui.md'), 'utf8')
    expect(guide).toContain(glossaryTable())
  })
})
