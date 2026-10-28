/** Literal match decoration only; result text always stays plain Vue text, never HTML. */
export function searchHighlight(text: string, query: string, caseSensitive: boolean) {
  if (!query) return [{ text, match: false }]
  const literal = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const expression = new RegExp(literal, caseSensitive ? 'gu' : 'giu')
  const parts: { text: string; match: boolean }[] = []
  let end = 0
  for (const match of text.matchAll(expression)) {
    const start = match.index
    if (start > end) parts.push({ text: text.slice(end, start), match: false })
    parts.push({ text: match[0], match: true })
    end = start + match[0].length
  }
  if (end < text.length) parts.push({ text: text.slice(end), match: false })
  return parts
}
