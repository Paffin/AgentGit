# Mutation verification

The unqualified symlink-worker score is rejected: test setup could fail before any assertion ran. Use the committed adapter, for example from `apps/web`:

```
mutator --no-coverage --mutate-all --max-workers 1 --timeout-factor 30 \
  --test-command 'node scripts/mutation-test.mjs src/lib/drafts.spec.ts src/composables/useFileDraft.spec.ts' \
  src/lib/drafts.ts src/composables/useFileDraft.ts
```

The adapter is tested both with actual passing tests and missing suites. In a mutation worker, a bootstrap failure remains a survivor, which fails the mutation gate; it is never credited as a kill. This is deliberately different from a standalone baseline, which fails with exit code 2.

The corrected draft run exposed missing timestamp-boundary and corrupted-record assertions. Those were added. A redundant version field in the composable was removed: serialization owns the format version.

## Reviewed survivors in previously shipped helpers

A re-run of PR #62's 35 helper sites caught 31 and left four equivalent changes. Additional tests now cover the exact upload count/size boundary, empty branches, absent entries, modal layering, missing app root and pre-existing inert/scroll state.

- `modalLayer.ts`, `lockPage`, `?? false` → `|| false`: `hasAttribute` returns a boolean, or optional chaining returns undefined. Both expressions therefore return the same boolean.
- `modalLayer.ts`, `lockPage`, fallback `false` → `true`: the fallback is evaluated only when the captured app root is null. Restoring inert on a null root has no effect; document scrolling is restored independently.
- `modalLayer.ts`, `zIndex`, `+ 1` → `+ 0`: all active modal layers shift down by the same integer. Relative modal ordering is unchanged; their base remains the dialog layer above the page. This is not an assertion about arbitrary consumer CSS.
- `uploads.ts`, `checkUploads`, `<` → `<=`: for a multiple-of-five input it performs one extra empty `Promise.all` batch. It makes no API calls and produces identical ordered results. Its extra scheduling turn is not counted as a product failure.

These are not reported as killed mutants or silently excluded. They remain visible in the tool's output; the review records the narrow reason and must be revisited when the functions change. The gate is not claimed green merely because the reviewer documented them.

## Current draft helpers

The final qualified run detects 37 of 40 selected mutations. Three survivors replace `storage ?? sessionStorage` with `storage || sessionStorage` in read/save/clear. The argument is a Storage interface object or undefined; every valid object is truthy, so these replacements are equivalent within the TypeScript contract. All three remain reported. No arbitrary false/null storage value is introduced merely to improve a score. The denied sessionStorage getter is exercised separately and must remain inside the error boundary.
