// Prints a compact list of the tests that failed (or were flaky) in a Playwright JSON report: one block per test, with
// the first lines of its error. For CI logs, where the full reporter output is long and the noise hides the cause.
import { existsSync, readFileSync } from 'node:fs'

const file = process.argv[2] ?? 'test-results/results.json'
if (!existsSync(file)) {
  process.stdout.write(`no report at ${file}\n`)
  process.exit(0)
}
const report = JSON.parse(readFileSync(file, 'utf8'))
// Playwright colours its messages: drop the escape sequences.
const strip = (s) => s.replace(new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*m`, 'g'), '')
let failed = 0
let flaky = 0
const lines = []

function walk(suite, trail) {
  const here = suite.title && !suite.file ? [...trail, suite.title] : trail
  for (const spec of suite.specs ?? []) {
    for (const t of spec.tests ?? []) {
      const status = t.status // expected | unexpected | flaky | skipped
      if (status !== 'unexpected' && status !== 'flaky') continue
      if (status === 'unexpected') failed++
      else flaky++
      const last = t.results.at(-1)
      const err = strip(last?.error?.message ?? last?.errors?.[0]?.message ?? '').split('\n')
      const head = err.filter((l) => l.trim()).slice(0, 7)
      lines.push(
        `${status === 'flaky' ? 'FLAKY ' : 'FAILED'} [${t.projectName}] ${spec.file} > ${[...here, spec.title].join(' > ')}`,
        ...head.map((l) => `        ${l.slice(0, 220)}`),
      )
    }
  }
  for (const child of suite.suites ?? []) walk(child, here)
}
for (const s of report.suites ?? []) walk(s, [])
process.stdout.write(`${failed} failed, ${flaky} flaky\n${lines.join('\n')}\n`)
