import { describe, expect, it } from 'vitest'
import { searchHighlight } from './searchHighlight'

describe('literal search decoration', () => {
  it('marks repeated text without treating symbols as expressions', () => {
    expect(searchHighlight('a.b A.B axb', 'a.b', false)).toEqual([
      { text: 'a.b', match: true },
      { text: ' ', match: false },
      { text: 'A.B', match: true },
      { text: ' axb', match: false },
    ])
  })
  it('preserves Unicode and markup as text and honors case', () => {
    const text = '🙂 <script>NEEDLE</script> needle'
    const parts = searchHighlight(text, 'needle', true)
    expect(parts.map((part) => part.text).join('')).toBe(text)
    expect(parts.filter((part) => part.match)).toEqual([{ text: 'needle', match: true }])
  })
})
