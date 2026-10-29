// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { formatSuggestion, parseSuggestion, replaceSuggestedRange } from './reviewSuggestion'

describe('range review suggestions', () => {
  it('round-trips explicit ranges while leaving single-line suggestions unchanged', () => {
    const body = formatSuggestion('Simplify this block.', 'replacement', 3)
    expect(body).toContain('```suggestion:-0+2\n')
    expect(parseSuggestion(body)).toEqual({
      explanation: 'Simplify this block.',
      replacement: 'replacement',
      lines: 3,
    })
    expect(formatSuggestion('', 'one', 1)).toBe('```suggestion\none\n```')
  })

  it('refuses unsupported or invalid ranges instead of treating them as a one-line edit', () => {
    for (const range of [0, -1, 1.5, 101])
      expect(() => formatSuggestion('', 'new', range)).toThrow()
    for (const header of ['suggestion:-1+2', 'suggestion:-0+100', 'suggestion:-0+-1'])
      expect(parseSuggestion(`\`\`\`${header}\nnew\n\`\`\``)).toBeNull()
  })

  it('replaces the exact original range and preserves BOM, CRLF and the final newline', () => {
    expect(
      replaceSuggestedRange('\uFEFFfirst\r\nsecond\r\nlast\r\n', 1, '\uFEFFfirst', 'new\nblock', 2),
    ).toBe('\uFEFFnew\r\nblock\r\nlast\r\n')
    expect(replaceSuggestedRange('first\nsecond\nlast', 2, 'second', '', 2)).toBe('first\n')
  })

  it('does not extend beyond the source or apply a mismatched starting line', () => {
    expect(() => replaceSuggestedRange('first\nlast\n', 2, 'last', 'new', 2)).toThrow(
      /no longer exists/,
    )
    expect(() => replaceSuggestedRange('first\nlast', 1, 'changed', 'new', 2)).toThrow(/differs/)
  })
})

it('covers exact range boundaries, malformed fences and empty original lines', () => {
  expect(parseSuggestion(formatSuggestion('', 'new', 100))?.lines).toBe(100)
  expect(parseSuggestion('```suggestion\n```\n```')).toBeNull()
  expect(parseSuggestion('```suggestion:-0+999999999999999999999999999\nnew\n```')).toBeNull()
  for (const line of [0, -1, NaN, 1.5])
    expect(() => replaceSuggestedRange('one', line, 'one', 'new')).toThrow()
  expect(replaceSuggestedRange('\nlast', 1, '', 'first')).toBe('first\nlast')
  expect(replaceSuggestedRange('\nlast', 2, 'last', 'final')).toBe('\nfinal')
  expect(replaceSuggestedRange('old\r', 1, 'old\r', 'new')).toBe('new')
  expect(replaceSuggestedRange('first\r\nlast', 2, 'last', 'new\nline')).toBe(
    'first\r\nnew\r\nline',
  )
  expect(replaceSuggestedRange('\uFEFFold\n', 1, '\uFEFFold', '\uFEFFnew')).toBe('\uFEFFnew\n')
})
