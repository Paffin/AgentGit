import type { Plugin } from 'vite'

/** Keep startup hints and lazy CSS; native imports own lazy JavaScript requests. */
export function preloadDependencies(
  _url: string,
  dependencies: string[],
  { hostType }: { hostType: 'html' | 'js' },
): string[] {
  return hostType === 'html'
    ? dependencies
    : dependencies.filter((dependency) => dependency.endsWith('.css'))
}

// Vite rewrites preload maps after the bundler computes chunk hashes. Hash the policy
// itself so changing it cannot reuse an immutable URL with different bytes.
export const preloadPolicy: Plugin = {
  name: 'ag-native-lazy-preloads',
  apply: 'build',
  augmentChunkHash: () => preloadDependencies.toString(),
}
