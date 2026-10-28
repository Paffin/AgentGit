export interface DiffLine {
  path: string
  side: 'old' | 'new'
  line: number
  text: string
}
export interface LineQuote extends DiffLine {
  base: string
  head: string
}

export function formatLineQuote(quote: LineQuote): string {
  const revision = quote.side === 'old' ? quote.base : quote.head
  return `${quote.path} — ${quote.side} line ${quote.line}\nRevision: ${revision || 'empty tree'}\nComparison head: ${quote.head}\n> ${quote.text}`
}
