// The one way an end-to-end test says "the component I need is not here". The twin of the Rust crate
// `crates/ag-testkit`, the Python module `scripts/ag_testkit.py` and the Go package `internal/testutil`
// (components.go); the rules are in docs/architecture/test-skips.md.
//
//   - the component is there: the test runs;
//   - it is missing and AG_REQUIRE_COMPONENTS names it (or is "all"): the test FAILS;
//   - otherwise the test SKIPS and leaves a machine-readable marker, which scripts/check_skips.py compares with the
//     reviewed list tests/skip-allowlist.toml. The marker is the skip message ("AG-SKIP component=... test=...
//     reason=...") and, when AG_SKIP_REPORT names a file, one JSON line appended to it.
import { appendFileSync } from 'node:fs'
import type { TestInfo } from '@playwright/test'

const MARKER = 'AG-SKIP'

/** Does the list ("a,b" / "a b" / "all") require `component`? `oci` and `oci:docker` cover each other. */
export function listRequires(list: string, component: string): boolean {
  const c = component.toLowerCase()
  return list
    .toLowerCase()
    .split(/[\s,]+/)
    .filter((t) => t !== '')
    .some((t) => t === 'all' || t === c || c.startsWith(`${t}:`) || t.startsWith(`${c}:`))
}

/** Is the absence of `component` a failure in this process? */
export function required(component: string): boolean {
  return listRequires(process.env.AG_REQUIRE_COMPONENTS ?? '', component)
}

/** The text of a skip: the same line in every language. */
export function marker(component: string, test: string, reason: string): string {
  return `${MARKER} component=${component} test=${test} reason=${reason.split(/\s+/).filter(Boolean).join(' ')}`
}

function record(component: string, test: string, reason: string): void {
  const path = process.env.AG_SKIP_REPORT
  if (!path) return
  const line = JSON.stringify({
    marker: MARKER,
    component,
    test,
    reason: reason.split(/\s+/).filter(Boolean).join(' '),
    lang: 'typescript',
  })
  try {
    appendFileSync(path, `${line}\n`)
  } catch {
    // the report is an aid; the marker in the skip message is still there
  }
}

/** Returns when `missing` is empty. Otherwise the test fails (the component is required) or is skipped. */
export function requireComponent(
  info: TestInfo,
  component: string,
  missing: string | null | undefined,
): void {
  if (!missing) return
  const test = info.titlePath.slice(1).join(' > ')
  if (required(component)) {
    throw new Error(
      `${component} is required (AG_REQUIRE_COMPONENTS) but unavailable, so "${test}" cannot run: ${missing}`,
    )
  }
  record(component, test, missing)
  info.skip(true, marker(component, test, missing))
}
