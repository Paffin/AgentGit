import type { Plugin } from 'vite'

/** Keep startup hints; a native lazy import fetches its own entry without a duplicate hint. */
export function preloadDependencies(
  url: string,
  dependencies: string[],
  { hostType }: { hostType: 'html' | 'js' },
): string[] {
  return hostType === 'html'
    ? dependencies
    : dependencies.filter((dependency) => dependency !== url)
}

// Vite rewrites preload maps after the bundler computes chunk hashes. Hash the policy
// itself so changing it cannot reuse an immutable URL with different bytes.
export const preloadPolicy: Plugin = {
  name: 'ag-native-lazy-preloads',
  apply: 'build',
  augmentChunkHash: () => preloadDependencies.toString(),
}
