// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { formatSuggestion, parseSuggestion, replaceSuggestedLine } from './reviewSuggestion'

describe('review suggestions', () => {
  it('round-trips code fences and an empty replacement', () => {
    for (const replacement of ['', 'new value', '```js\nexample\n```']) {
      expect(parseSuggestion(formatSuggestion('Explain the change.', replacement))).toEqual({
        explanation: 'Explain the change.',
        replacement,
      })
    }
    expect(parseSuggestion('An ordinary comment')).toBeNull()
    expect(parseSuggestion('Reason\r\n\r\n```suggestion\r\nnew\r\n```\r\n')).toEqual({
      explanation: 'Reason',
      replacement: 'new',
    })
  })

  it('preserves Unicode, line endings, BOM and untouched content', () => {
    expect(replaceSuggestedLine('\uFEFFfirst\r\nold\r\nпоследняя', 2, 'old', 'new\nvalue')).toBe(
      '\uFEFFfirst\r\nnew\r\nvalue\r\nпоследняя',
    )
    expect(replaceSuggestedLine('first\nlast', 2, 'last', 'final')).toBe('first\nfinal')
    expect(replaceSuggestedLine('\uFEFFold\n', 1, '\uFEFFold', 'new')).toBe('\uFEFFnew\n')
  })

  it('deletes only the selected logical line', () => {
    expect(replaceSuggestedLine('first\nold\nlast\n', 2, 'old', '')).toBe('first\nlast\n')
    expect(replaceSuggestedLine('only\n', 1, 'only', '')).toBe('')
    expect(replaceSuggestedLine('first\nlast', 2, 'last', '')).toBe('first\n')
  })

  it('rejects stale, absent and binary source without a fuzzy match', () => {
    expect(() => replaceSuggestedLine('changed\n', 1, 'old', 'new')).toThrow()
    expect(() => replaceSuggestedLine('one\n', 2, '', 'new')).toThrow()
    expect(() => replaceSuggestedLine('one\0', 1, 'one\0', 'new')).toThrow()
  })
})
