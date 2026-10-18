import { describe, expect, it } from 'vitest'
import { compareBuilds, parseVersion, protocolMismatch } from './version'

describe('versions', () => {
  it('parses semantic versions and nothing else', () => {
    expect(parseVersion('0.1.0')).toEqual({ major: 0, minor: 1, patch: 0, pre: '' })
    expect(parseVersion('v1.2.3-rc.1+abc')).toEqual({ major: 1, minor: 2, patch: 3, pre: 'rc.1' })
    for (const bad of ['', '1.2', 'one.two.three', '1.2.3.4', 'latest']) {
      expect(parseVersion(bad), bad).toBeNull()
    }
  })

  it('same, compatible, mismatch, unknown', () => {
    expect(compareBuilds('0.1.0', '0.1.0')).toEqual({ kind: 'same' })
    expect(compareBuilds('0.1.0', '0.1.7')).toEqual({ kind: 'compatible' })
    expect(compareBuilds('0.1.0-rc.1', '0.1.0')).toEqual({ kind: 'compatible' })
    expect(compareBuilds('0.1.0', '0.2.0')).toEqual({
      kind: 'mismatch',
      ui: '0.1.0',
      server: '0.2.0',
      uiNewer: false,
    })
    expect(compareBuilds('1.0.0', '0.9.9')).toMatchObject({ kind: 'mismatch', uiNewer: true })
    expect(compareBuilds('0.3.0', '0.2.9')).toMatchObject({ kind: 'mismatch', uiNewer: true })
    // Not readable: say nothing instead of guessing.
    expect(compareBuilds(null, '0.1.0')).toEqual({ kind: 'unknown' })
    expect(compareBuilds('0.1.0', '')).toEqual({ kind: 'unknown' })
    expect(compareBuilds('dev', '0.1.0')).toEqual({ kind: 'unknown' })
  })

  it('protocol', () => {
    expect(protocolMismatch('agwp.v1')).toBeNull()
    expect(protocolMismatch('')).toBeNull()
    expect(protocolMismatch(undefined)).toBeNull()
    expect(protocolMismatch('agwp.v2')).toBe('agwp.v2')
  })
})
