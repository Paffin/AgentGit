import { describe, expect, it } from 'vitest'
import {
  firstLine,
  formatBytes,
  formatCountdown,
  formatDuration,
  formatIso,
  formatRelative,
  listSummary,
  plural,
  shortId,
} from './format'

describe('shortId', () => {
  it('keeps the prefix of typed ids and shortens the rest', () => {
    expect(shortId('prop_8d079ff9771014b419e9')).toBe('prop_8d079ff9')
    expect(shortId('prop_8d079ff9771014b419e9', 4)).toBe('prop_8d07')
    expect(shortId('a0719cff81ed89a11a58f8cf5d96d826', 10)).toBe('a0719cff81')
    expect(shortId('')).toBe('')
  })
})

describe('formatBytes', () => {
  it('scales and rounds', () => {
    expect(formatBytes(0)).toBe('0 B')
    expect(formatBytes(1023)).toBe('1023 B')
    expect(formatBytes(1024)).toBe('1.00 KB')
    expect(formatBytes(1536)).toBe('1.50 KB')
    expect(formatBytes(10 * 1024 * 1024)).toBe('10.0 MB')
    expect(formatBytes(512 * 1024 * 1024)).toBe('512 MB')
  })
  it('refuses nonsense', () => {
    expect(formatBytes(-1)).toBe('–')
    expect(formatBytes(Number.NaN)).toBe('–')
  })
})

describe('formatRelative', () => {
  const now = 1_700_000_000_000
  it('reads naturally in the past', () => {
    expect(formatRelative(now - 1000, now)).toBe('just now')
    expect(formatRelative(now - 12_000, now)).toBe('12s ago')
    expect(formatRelative(now - 5 * 60_000, now)).toBe('5m ago')
    expect(formatRelative(now - 3 * 3600_000, now)).toBe('3h ago')
    expect(formatRelative(now - 2 * 86_400_000, now)).toBe('2d ago')
  })
  it('and in the future', () => {
    expect(formatRelative(now + 90_000, now)).toBe('in 2m')
  })
  it('says never for zero and falls back to a date for old things', () => {
    expect(formatRelative(0, now)).toBe('never')
    expect(formatRelative(now - 90 * 86_400_000, now)).toMatch(/^\d{4}-\d{2}-\d{2}$/)
  })
})

describe('formatDuration / formatCountdown', () => {
  it('uses the two most significant units', () => {
    expect(formatDuration(0)).toBe('0s')
    expect(formatDuration(42_000)).toBe('42s')
    expect(formatDuration(4 * 60_000 + 5000)).toBe('4m 05s')
    expect(formatDuration(3600_000 + 5 * 60_000)).toBe('1h 05m')
    expect(formatDuration(3 * 86_400_000 + 4 * 3600_000)).toBe('3d 4h')
    expect(formatDuration(-5)).toBe('0s')
  })
  it('counts down and then says expired', () => {
    const now = 1_000_000
    expect(formatCountdown(now + 90_000, now)).toBe('1m 30s')
    expect(formatCountdown(now, now)).toBe('expired')
    expect(formatCountdown(now - 1, now)).toBe('expired')
  })
})

describe('small helpers', () => {
  it('plural', () => {
    expect(plural(1, 'file')).toBe('1 file')
    expect(plural(2, 'file')).toBe('2 files')
    expect(plural(0, 'repository', 'repositories')).toBe('0 repositories')
    expect(plural(1234, 'event')).toBe('1,234 events')
  })
  it('firstLine / listSummary / formatIso', () => {
    expect(firstLine('a\nb')).toBe('a')
    expect(firstLine('only')).toBe('only')
    expect(listSummary(['a', 'b'])).toBe('a, b')
    expect(listSummary(['a', 'b', 'c', 'd', 'e'], 2)).toBe('a, b and 3 more')
    expect(formatIso(0)).toBe('')
    expect(formatIso(1_700_000_000_000)).toBe('2023-11-14T22:13:20.000Z')
  })
})
