/** A fenced suggestion stays readable through CLI/SDKs that display plain thread text. */
export interface ReviewSuggestion {
  explanation: string
  replacement: string
}

export function formatSuggestion(explanation: string, replacement: string): string {
  const runs = `${explanation}\n${replacement}`.match(/`+/g) || []
  const fence = '`'.repeat(Math.max(3, ...runs.map((run) => run.length + 1)))
  return `${explanation.trim()}\n\n${fence}suggestion\n${replacement}\n${fence}`.trimStart()
}

export function parseSuggestion(body: string): ReviewSuggestion | null {
  const text = body.replace(/\r\n/g, '\n').trimEnd()
  const match = /(?:^|\n)(`{3,})suggestion\n([\s\S]*)\n\1$/.exec(text)
  if (!match) return null
  const fence = match[1]!
  const replacement = match[2]!
  if (replacement.split('\n').some((line) => line.startsWith(fence))) return null
  return { explanation: text.slice(0, match.index).trim(), replacement }
}

interface LineSpan {
  start: number
  end: number
  next: number
}

function locateLine(source: string, line: number): LineSpan {
  if (!Number.isSafeInteger(line) || line < 1) throw new Error('Select a valid text line.')
  let start = 0
  for (let n = 1; n < line; n++) {
    const newline = source.indexOf('\n', start)
    if (newline < 0) throw new Error('The suggested line no longer exists.')
    start = newline + 1
  }
  if (start >= source.length) throw new Error('The suggested line no longer exists.')
  const newline = source.indexOf('\n', start)
  if (newline < 0) return { start, end: source.length, next: source.length }
  return { start, end: newline, next: newline + 1 }
}

function inspectedLine(source: string, span: LineSpan) {
  const raw = source.slice(span.start, span.end)
  if (span.next > span.end && raw.endsWith('\r')) return raw.slice(0, -1)
  return raw
}

function separatorFor(source: string, span: LineSpan): string {
  if (span.next > span.end) return source[span.end - 1] === '\r' ? '\r\n' : '\n'
  return source.includes('\r\n') ? '\r\n' : '\n'
}

function spliceLine(source: string, span: LineSpan, replacement: string): string {
  let prefix = source.slice(0, span.start)
  if (span.start === 0 && source.startsWith('\uFEFF')) {
    prefix = '\uFEFF'
    if (replacement.startsWith('\uFEFF')) replacement = replacement.slice(1)
  }
  const suffix = source.slice(span.next)
  if (replacement === '') return prefix + suffix
  const separator = separatorFor(source, span)
  const inserted = replacement.replace(/\r\n/g, '\n').replace(/\n/g, separator)
  const ending = span.next > span.end ? separator : ''
  return prefix + inserted + ending + suffix
}

/** Replace exactly one anchored line, preserving surrounding content and newline style. */
export function replaceSuggestedLine(
  source: string,
  line: number,
  expected: string,
  replacement: string,
): string {
  if (source.includes('\0') || replacement.includes('\0'))
    throw new Error('This suggestion requires a text file.')
  const span = locateLine(source, line)
  if (inspectedLine(source, span) !== expected)
    throw new Error(
      'The source line differs from the reviewed line. Inspect the current change request.',
    )
  return spliceLine(source, span, replacement)
}
