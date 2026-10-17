// A strict JSON parser that remembers where every value came from, so the policy editor can point at the
// exact line of a mistake. (JSON.parse reports a character offset at best, and nothing for valid JSON.)

export interface Pos {
  line: number
  col: number
  offset: number
}

export interface JsonError {
  message: string
  line: number
  col: number
  offset: number
}

export type JsonValue =
  null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue }

export interface JsonParseResult {
  value?: JsonValue
  /** Position of every key and array element, by path like `approvals.min` or `trusted_issuers[0].identity`. */
  positions: Map<string, Pos>
  /** Keys that appear twice in one object. */
  duplicates: { path: string; pos: Pos }[]
  error?: JsonError
}

class Stop extends Error {
  readonly info: JsonError
  constructor(info: JsonError) {
    super(info.message)
    this.info = info
  }
}

export function parseJsonWithPositions(text: string): JsonParseResult {
  const positions = new Map<string, Pos>()
  const duplicates: { path: string; pos: Pos }[] = []
  let i = 0

  const posAt = (offset: number): Pos => {
    let line = 1
    let last = -1
    for (let k = 0; k < offset && k < text.length; k++) {
      if (text.charCodeAt(k) === 10) {
        line++
        last = k
      }
    }
    return { line, col: offset - last, offset }
  }
  const fail = (message: string, at = i): never => {
    const p = posAt(at)
    throw new Stop({ message, line: p.line, col: p.col, offset: at })
  }
  const ws = () => {
    while (i < text.length && ' \t\r\n'.includes(text.charAt(i))) i++
  }
  const describe = (c: string | undefined) => (c === undefined ? 'end of input' : `“${c}”`)

  function value(path: string): JsonValue {
    ws()
    const c = text[i]
    if (c === '{') return object(path)
    if (c === '[') return array(path)
    if (c === '"') return string()
    if (c === '-' || (c !== undefined && c >= '0' && c <= '9')) return number()
    for (const [word, v] of [
      ['true', true],
      ['false', false],
      ['null', null],
    ] as const) {
      if (text.startsWith(word, i)) {
        i += word.length
        return v
      }
    }
    return fail(`Unexpected ${describe(c)}; expected a value`)
  }

  function object(path: string): JsonValue {
    const out: { [key: string]: JsonValue } = {}
    i++ // {
    ws()
    if (text[i] === '}') {
      i++
      return out
    }
    for (;;) {
      ws()
      if (text[i] !== '"')
        fail(`Unexpected ${describe(text[i])}; expected a property name in double quotes`)
      const keyAt = i
      const key = string()
      const childPath = path ? `${path}.${key}` : key
      const p = posAt(keyAt)
      if (Object.hasOwn(out, key)) duplicates.push({ path: childPath, pos: p })
      positions.set(childPath, p)
      ws()
      if (text[i] !== ':')
        fail(`Unexpected ${describe(text[i])}; expected “:” after the property name`)
      i++
      out[key] = value(childPath)
      ws()
      if (text[i] === ',') {
        i++
        continue
      }
      if (text[i] === '}') {
        i++
        return out
      }
      fail(`Unexpected ${describe(text[i])}; expected “,” or “}”`)
    }
  }

  function array(path: string): JsonValue {
    const out: JsonValue[] = []
    i++ // [
    ws()
    if (text[i] === ']') {
      i++
      return out
    }
    for (;;) {
      ws()
      positions.set(`${path}[${out.length}]`, posAt(i))
      out.push(value(`${path}[${out.length}]`))
      ws()
      if (text[i] === ',') {
        i++
        continue
      }
      if (text[i] === ']') {
        i++
        return out
      }
      fail(`Unexpected ${describe(text[i])}; expected “,” or “]”`)
    }
  }

  function string(): string {
    const start = i
    i++ // opening quote
    let out = ''
    for (;;) {
      if (i >= text.length) fail('The string is not closed', start)
      const c = text.charAt(i)
      if (c === '"') {
        i++
        return out
      }
      if (c === '\n') fail('Strings cannot contain a raw line break', i)
      if (c === '\\') {
        const e = text.charAt(i + 1)
        const map: Record<string, string> = {
          '"': '"',
          '\\': '\\',
          '/': '/',
          b: '\b',
          f: '\f',
          n: '\n',
          r: '\r',
          t: '\t',
        }
        if (e in map) {
          out += map[e]
          i += 2
        } else if (e === 'u' && /^[0-9a-fA-F]{4}$/.test(text.slice(i + 2, i + 6))) {
          out += String.fromCharCode(parseInt(text.slice(i + 2, i + 6), 16))
          i += 6
        } else {
          fail('Invalid escape sequence in a string', i)
        }
      } else {
        out += c
        i++
      }
    }
  }

  function number(): number {
    const m = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(text.slice(i))
    if (!m) return fail('Invalid number')
    i += m[0].length
    return Number(m[0])
  }

  try {
    ws()
    if (i >= text.length) fail('The document is empty')
    const v = value('')
    ws()
    if (i < text.length) fail(`Unexpected ${describe(text[i])} after the end of the document`)
    return { value: v, positions, duplicates }
  } catch (e) {
    if (e instanceof Stop) return { positions, duplicates, error: e.info }
    throw e
  }
}
