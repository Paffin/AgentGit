import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'
import { describe, expect, it } from 'vitest'

// The UI renders text the server does not control (titles, diffs, file contents, rationales, identities).
// These checks make the safe way the only way: no markup injection primitives anywhere in the source.

const root = join(__dirname, '..')

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    if (statSync(p).isDirectory()) walk(p, out)
    else out.push(p)
  }
  return out
}

const source = walk(join(root, 'src')).filter(
  (f) => /\.(vue|ts)$/.test(f) && !/\.spec\.ts$/.test(f) && !f.endsWith('schema.gen.ts'),
)
const text = (f: string) => readFileSync(f, 'utf8')
const rel = (f: string) => relative(root, f)

describe('source hygiene', () => {
  it('finds the source files', () => {
    expect(source.length).toBeGreaterThan(80)
  })

  it.each([
    ['v-html', /v-html/],
    ['innerHTML / outerHTML', /\b(innerHTML|outerHTML)\b/],
    ['insertAdjacentHTML', /insertAdjacentHTML/],
    ['document.write', /document\.write/],
    ['eval()', /\beval\s*\(/],
    ['new Function', /new\s+Function\s*\(/],
    ['javascript: URLs', /javascript:/i],
    ['srcdoc', /\bsrcdoc\b/],
  ])('never uses %s', (_name, pattern) => {
    const offenders = source.filter((f) => pattern.test(text(f))).map(rel)
    expect(offenders).toEqual([])
  })

  it('has no inline event-handler attributes in templates (the CSP would block them anyway)', () => {
    const offenders = source
      .filter((f) => f.endsWith('.vue'))
      .filter((f) =>
        /<[a-z][^>]*\son(click|error|load|mouseover|focus|submit|change|input)\s*=/i.test(text(f)),
      )
      .map(rel)
    expect(offenders).toEqual([])
  })

  it('never logs: tokens must not reach a console', () => {
    const offenders = source
      .filter((f) => /\bconsole\.(log|info|warn|error|debug|trace)\b/.test(text(f)))
      .map(rel)
    expect(offenders).toEqual([])
  })

  it('keeps the token in sessionStorage only (never localStorage, never a URL)', () => {
    const auth = text(join(root, 'src/stores/auth.ts'))
    expect(auth).toContain("writeStore('session', TOKEN_KEY")
    expect(auth).not.toMatch(/writeStore\('local'/)
    const offenders = source
      .filter(
        (f) =>
          /localStorage\.[a-z]+\([^)]*token/i.test(text(f)) ||
          /\?token=|&token=|[?&]access_token/.test(text(f)),
      )
      .map(rel)
    expect(offenders).toEqual([])
  })

  it('does not load anything from another origin (the server CSP allows only itself)', () => {
    const offenders = source
      .concat([join(root, 'index.html')])
      .filter(
        (f) =>
          /(src|href)\s*=\s*["']https?:\/\//i.test(text(f)) ||
          /@import\s+url\(["']?https?:/i.test(text(f)) ||
          /fonts\.googleapis|cdn\./i.test(text(f)),
      )
      .map(rel)
    expect(offenders).toEqual([])
  })

  it('ships index.html without inline scripts', () => {
    const html = text(join(root, 'index.html'))
    const scripts = [...html.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/g)]
    for (const [, attrs, body] of scripts) {
      expect(attrs).toMatch(/\bsrc=/)
      expect(body?.trim()).toBe('')
    }
  })
})
