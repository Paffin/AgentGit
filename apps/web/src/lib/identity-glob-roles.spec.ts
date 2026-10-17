import { describe, expect, it } from 'vitest'
import { isHuman, parseIdentity, samePrincipal } from './identity'
import { matchGlob, repoPatternMatches, scopesMayOverlap } from './glob'
import { mayDo, parseRoleGrants, requirementText, rolesForRepo, satisfies } from './roles'

describe('identities', () => {
  it('parses kind, name, principal and run', () => {
    expect(parseIdentity('user:alice')).toMatchObject({
      kind: 'user',
      name: 'alice',
      principal: 'user:alice',
      run: '',
    })
    expect(parseIdentity('agent:refactor-bot#run_ab12')).toMatchObject({
      kind: 'agent',
      name: 'refactor-bot',
      principal: 'agent:refactor-bot',
      run: 'run_ab12',
    })
    expect(parseIdentity('service:ci').kind).toBe('service')
    expect(parseIdentity('system:recovery').kind).toBe('system')
  })
  it('keeps unknown shapes verbatim instead of guessing', () => {
    expect(parseIdentity('weird')).toMatchObject({ kind: 'unknown', name: 'weird' })
    expect(parseIdentity('robot:x').kind).toBe('unknown')
  })
  it('compares principals, so a new run is the same agent', () => {
    expect(samePrincipal('agent:bot#run_1', 'agent:bot#run_2')).toBe(true)
    expect(samePrincipal('user:a', 'user:b')).toBe(false)
    expect(isHuman('user:a')).toBe(true)
    expect(isHuman('agent:a')).toBe(false)
  })
})

describe('globs', () => {
  it('matches * within a component and ** across components', () => {
    expect(matchGlob('src/*.ts', 'src/a.ts')).toBe(true)
    expect(matchGlob('src/*.ts', 'src/x/a.ts')).toBe(false)
    expect(matchGlob('src/**', 'src/x/y/a.ts')).toBe(true)
    expect(matchGlob('docs/**', 'src/a.ts')).toBe(false)
    expect(matchGlob('**/*.md', 'README.md')).toBe(true)
    expect(matchGlob('**/*.md', 'a/b/README.md')).toBe(true)
    expect(matchGlob('.github/**', '.github/workflows/ci.yml')).toBe(true)
  })
  it('escapes regex metacharacters in literals', () => {
    expect(matchGlob('a+b/(c).ts', 'a+b/(c).ts')).toBe(true)
    expect(matchGlob('a.ts', 'aXts')).toBe(false)
  })
  it('mirrors the server rule for overlapping scopes: literal prefixes decide', () => {
    expect(scopesMayOverlap('src/**', 'src/auth/**')).toBe(true)
    expect(scopesMayOverlap('src/auth/**', 'src/**')).toBe(true)
    expect(scopesMayOverlap('docs/**', 'src/**')).toBe(false)
    expect(scopesMayOverlap('**', 'src/**')).toBe(true)
    expect(scopesMayOverlap('src/auth/a.ts', 'src/auth/b.ts')).toBe(false)
  })
  it('matches repository patterns', () => {
    expect(repoPatternMatches('*', 'acme/web')).toBe(true)
    expect(repoPatternMatches('acme/*', 'acme/web')).toBe(true)
    expect(repoPatternMatches('acme/*', 'other/web')).toBe(false)
    expect(repoPatternMatches('acme/web', 'acme/web')).toBe(true)
    expect(repoPatternMatches('acme/web', 'acme/webapp')).toBe(false)
  })
})

describe('roles', () => {
  const grants = parseRoleGrants([
    'acme/*:writer',
    '*:reader',
    'acme/web:reviewer',
    'garbage',
    'x/y:wizard',
  ])

  it('parses grants and drops what it does not understand', () => {
    expect(grants.map((g) => `${g.pattern}:${g.role}`)).toEqual([
      'acme/*:writer',
      '*:reader',
      'acme/web:reviewer',
    ])
  })
  it('collects the roles that apply to a repository', () => {
    expect([...rolesForRepo(grants, 'acme/web')].sort()).toEqual(['reader', 'reviewer', 'writer'])
    expect([...rolesForRepo(grants, 'other/x')]).toEqual(['reader'])
  })
  it('lets admin do everything and any role read', () => {
    expect(satisfies(new Set(['admin']), 'integrator')).toBe(true)
    expect(satisfies(new Set(['reviewer']), 'reader')).toBe(true)
    expect(satisfies(new Set(['reviewer']), 'integrator')).toBe(false)
    expect(satisfies(new Set(), 'reader')).toBe(false)
  })
  it('maps UI actions to roles', () => {
    const writer = new Set(['writer' as const])
    expect(mayDo(writer, 'requestValidation')).toBe(true)
    expect(mayDo(writer, 'review')).toBe(false)
    expect(mayDo(writer, 'integrate')).toBe(false)
    expect(mayDo(writer, 'enqueue')).toBe(true)
    expect(mayDo(new Set(['integrator' as const]), 'integrate')).toBe(true)
    expect(mayDo(new Set(['reviewer' as const]), 'review')).toBe(true)
    expect(mayDo(new Set(['admin' as const]), 'createRepo')).toBe(true)
    expect(requirementText('review')).toBe('Requires the reviewer role')
    expect(requirementText('enqueue')).toBe('Requires the integrator or writer role')
  })
})
