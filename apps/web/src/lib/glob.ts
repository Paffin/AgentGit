// Path globs as AgentGit uses them (docs/guides/policy.md): `*` inside one path component, `**` across
// components. Overlap detection mirrors the server's conservative rule: compare literal prefixes.

function literalPrefix(glob: string): string {
  const i = glob.search(/[*?[{]/)
  return i === -1 ? glob : glob.slice(0, i)
}

/**
 * True unless the literal prefixes prove two scopes cannot match a common path. It may report an overlap
 * that is not real; it never misses one (same contract as the server's lease check).
 */
export function scopesMayOverlap(a: string, b: string): boolean {
  const pa = literalPrefix(a)
  const pb = literalPrefix(b)
  return pa.startsWith(pb) || pb.startsWith(pa)
}

export function globToRegExp(glob: string): RegExp {
  let re = ''
  for (let i = 0; i < glob.length; i++) {
    const c = glob.charAt(i)
    if (c === '*') {
      if (glob.charAt(i + 1) === '*') {
        i++
        if (glob.charAt(i + 1) === '/') {
          i++
          re += '(?:.*/)?'
        } else {
          re += '.*'
        }
      } else {
        re += '[^/]*'
      }
    } else if (c === '?') {
      re += '[^/]'
    } else {
      re += c.replace(/[.+^${}()|[\]\\]/g, '\\$&')
    }
  }
  return new RegExp(`^${re}$`)
}

export function matchGlob(glob: string, path: string): boolean {
  return globToRegExp(glob).test(path)
}

/** Repository patterns in roles and scopes: `acme/webapp`, `acme/*`, `*`. */
export function repoPatternMatches(pattern: string, repo: string): boolean {
  if (pattern === '*') return true
  if (!pattern.includes('*')) return pattern === repo
  const re = new RegExp(
    '^' +
      pattern
        .split('*')
        .map((p) => p.replace(/[.+^${}()|[\]\\?]/g, '\\$&'))
        .join('.*') +
      '$',
  )
  return re.test(repo)
}
