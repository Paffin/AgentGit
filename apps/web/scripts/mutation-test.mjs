// Run actual tests against the worker tree, without following source symlinks back
// to unmutated files or crediting a failed bootstrap as a killed mutation.
import { spawnSync } from 'node:child_process'
import {
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  unlinkSync,
} from 'node:fs'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { stripVTControlCharacters } from 'node:util'

const source = realpathSync(resolve(dirname(fileURLToPath(import.meta.url)), '..'))
const working = realpathSync(process.cwd())
const worker = working !== source
const excluded = new Set([
  'node_modules',
  'target',
  'dist',
  'test-results',
  'playwright-report',
  '.metrics',
  '.git',
  'coverage',
  '.runtime',
  '.cache',
])

function materialize(from, to) {
  const current = lstatSync(to, { throwIfNoEntry: false })
  if (current?.isSymbolicLink()) unlinkSync(to)
  if (lstatSync(from).isDirectory()) {
    mkdirSync(to, { recursive: true })
    for (const name of readdirSync(from)) {
      if (!excluded.has(name)) materialize(join(from, name), join(to, name))
    }
  } else if (!existsSync(to)) {
    copyFileSync(from, to)
  }
}

let config = join(source, 'vite.config.ts')
if (worker) {
  const workerRoot = join(source, 'target', 'mutation-workers')
  const inside = relative(workerRoot, working)
  if (!inside || inside.startsWith('..') || isAbsolute(inside))
    throw new Error('Unexpected mutation worker directory')
  // A symlinked test resolves relative imports back into the original tree. Copy
  // project files into the overlay, preserving the real file containing the mutant.
  // Dependencies and generated output stay shared or excluded.
  materialize(source, working)
  config = join(working, 'vite.config.ts')
}
const result = spawnSync(
  process.execPath,
  [
    join(source, 'node_modules/vitest/vitest.mjs'),
    'run',
    '--config',
    config,
    ...process.argv.slice(2),
  ],
  { encoding: 'utf8' },
)
const output = `${result.stdout ?? ''}${result.stderr ?? ''}`
process.stdout.write(output)
const plain = stripVTControlCharacters(output)
if (!/Tests\s+[^\n]*\d+ (?:passed|failed)/.test(plain)) {
  process.stderr.write(
    'UNVERIFIED: no actual test results; do not credit this as a killed mutation.\n',
  )
  // The external runner treats every nonzero exit as a kill. Conservatively leave a worker
  // mutation alive (which fails its gate), while a broken baseline exits unsuccessfully.
  process.exit(worker ? 0 : 2)
}
process.exit(result.status ?? 2)
