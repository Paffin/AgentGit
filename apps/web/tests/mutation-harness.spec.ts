// @vitest-environment node
import { spawnSync } from 'node:child_process'
import { stripVTControlCharacters } from 'node:util'
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const script = join(root, 'scripts/mutation-test.mjs')
function run(cwd: string, ...args: string[]) {
  return spawnSync(process.execPath, [script, ...args], {
    cwd,
    encoding: 'utf8',
    timeout: 20_000,
    env: { ...process.env, FORCE_COLOR: '1', NO_COLOR: undefined },
  })
}
describe('mutation harness qualification', () => {
  it('runs actual tests in the ordinary project', () => {
    const result = run(root, 'src/lib/drafts.spec.ts')
    expect(result.status, result.stderr).toBe(0)
    expect(stripVTControlCharacters(result.stdout)).toMatch(/Tests\s+\d+ passed/)
  }, 25_000)
  it('refuses a baseline with no tests', () => {
    const result = run(root, 'this-test-does-not-exist.spec.ts')
    expect(result.status).toBe(2)
    expect(result.stderr).toContain('UNVERIFIED')
  }, 25_000)
  it('does not credit a worker bootstrap failure as a killed mutant', () => {
    const directory = join(root, 'target/mutation-workers')
    mkdirSync(directory, { recursive: true })
    const worker = mkdtempSync(join(directory, 'harness-check-'))
    try {
      const result = run(worker, 'this-test-does-not-exist.spec.ts')
      expect(result.status).toBe(0) // The external runner must classify it as a survivor, failing its gate.
      expect(result.stderr).toContain('UNVERIFIED')
    } finally {
      rmSync(worker, { recursive: true, force: true })
    }
  }, 25_000)
})
