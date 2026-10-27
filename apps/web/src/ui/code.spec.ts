import { describe, expect, it } from 'vitest'
import { mount } from '@vue/test-utils'
import type { FileDiff } from '@/api/types'
import type { StateInfo } from '@/lib/fourstate'
import CodeBlock from './CodeBlock.vue'
import DiffFile from './DiffFile.vue'
import DiffViewer from './DiffViewer.vue'
import LiveIndicator from './LiveIndicator.vue'
import StatePill from './StatePill.vue'

describe('CodeBlock', () => {
  it('shows each line as text, numbered by CSS so copying skips the numbers', () => {
    const w = mount(CodeBlock, {
      props: { code: 'one\n<b>two</b>\nthree\n', label: 'Contents of a.txt' },
    })
    const lines = w.findAll('.line')
    expect(lines.map((l) => l.text())).toEqual(['one', '<b>two</b>', 'three'])
    expect(w.find('b').exists()).toBe(false)
    expect(w.find('pre').attributes('aria-label')).toBe('Contents of a.txt')
    expect(w.find('pre').attributes('tabindex')).toBe('0') // keyboard-scrollable
    expect(lines[0]!.classes()).toContain('numbered')
  })
  it('offers a copy button, and can drop line numbers', () => {
    const w = mount(CodeBlock, { props: { code: 'x', label: 'x', lineNumbers: false } })
    expect(w.find('[aria-label="Copy contents"]').exists()).toBe(true)
    expect(w.find('.line').classes()).not.toContain('numbered')
  })
  it('switches to a virtual list for huge files', () => {
    const code = Array.from({ length: 20_000 }, (_, i) => `line ${i}`).join('\n')
    const w = mount(CodeBlock, { props: { code, label: 'big' }, attachTo: document.body })
    expect(w.find('pre').exists()).toBe(false)
    expect(w.find('[role="list"]').exists()).toBe(true)
    expect(w.findAll('.vline').length).toBeLessThan(60)
    w.unmount()
  })
})

const file = (over: Partial<FileDiff> = {}): FileDiff => ({
  path: 'src/auth/session.ts',
  status: 'modified',
  binary: false,
  additions: 2,
  deletions: 1,
  patch:
    '--- a/src/auth/session.ts\n+++ b/src/auth/session.ts\n@@ -1,3 +1,4 @@ export\n keep\n-old\n+new\n+newer\n tail\n',
  ...over,
})

describe('DiffFile', () => {
  it('renders hunks with both line-number columns and added/removed markers', () => {
    const w = mount(DiffFile, { props: { file: file() } })
    expect(w.find('.path').text()).toBe('src/auth/session.ts')
    const rows = w.findAll('.row')
    expect(rows.map((r) => r.classes().find((c) => c.startsWith('k-')))).toEqual([
      'k-ctx',
      'k-del',
      'k-add',
      'k-add',
      'k-ctx',
    ])
    expect(rows[1]!.findAll('.no').map((n) => n.text())).toEqual(['2', ''])
    expect(rows[2]!.findAll('.no').map((n) => n.text())).toEqual(['', '2'])
    expect(rows[1]!.find('.sr-only').text()).toContain('removed')
    expect(rows[2]!.find('.sr-only').text()).toContain('added')
    expect(w.find('.hunk-head').text()).toContain('@@ -1,3 +1,4 @@')
    expect(w.find('.hunk-head').text()).toContain('export')
    expect(w.text()).toContain('+2')
    expect(w.text()).toContain('−1')
  })

  it('never turns file content into markup', () => {
    const evil = '<img src=x onerror="window.__pwned=1"><script>window.__pwned=2</script>'
    const w = mount(DiffFile, {
      props: { file: file({ path: evil, patch: `@@ -0,0 +1 @@\n+${evil}\n` }) },
    })
    expect(w.find('img').exists()).toBe(false)
    expect(w.find('script').exists()).toBe(false)
    expect(w.text()).toContain(evil)
    expect((window as unknown as { __pwned?: number }).__pwned).toBeUndefined()
  })

  it('does not show binary content', () => {
    const w = mount(DiffFile, {
      props: { file: file({ binary: true, patch: '', additions: 0, deletions: 0 }) },
    })
    expect(w.text()).toContain('Binary file')
    expect(w.findAll('.row')).toHaveLength(0)
  })

  it('says so when there are no textual changes', () => {
    expect(mount(DiffFile, { props: { file: file({ patch: '' }) } }).text()).toContain(
      'No textual changes',
    )
  })

  it('collapses huge diffs until asked, and files can be toggled', async () => {
    const body = Array.from({ length: 700 }, (_, i) => `+line ${i}`).join('\n')
    const w = mount(DiffFile, {
      props: { file: file({ patch: `@@ -0,0 +1,700 @@\n${body}\n`, additions: 700 }) },
    })
    expect(w.findAll('.row')).toHaveLength(0)
    expect(w.text()).toContain('701 lines hidden')
    await w.find('.collapsed-note button').trigger('click')
    expect(w.findAll('.row')).toHaveLength(700)

    const small = mount(DiffFile, { props: { file: file() } })
    const toggle = small.find('button.toggle')
    expect(toggle.attributes('aria-expanded')).toBe('true')
    await toggle.trigger('click')
    expect(toggle.attributes('aria-expanded')).toBe('false')
    expect(small.findAll('.row')).toHaveLength(0)
  })

  it('labels the status of added and deleted files', () => {
    expect(
      mount(DiffFile, { props: { file: file({ status: 'added' }) } })
        .find('.badge')
        .classes(),
    ).toContain('t-ok')
    expect(
      mount(DiffFile, { props: { file: file({ status: 'deleted' }) } })
        .find('.badge')
        .classes(),
    ).toContain('t-bad')
  })
})

describe('DiffViewer', () => {
  it('summarises and lists every file', () => {
    const w = mount(DiffViewer, {
      props: { files: [file(), file({ path: 'b.ts', additions: 5, deletions: 0 })] },
    })
    expect(w.text()).toContain('2 files changed')
    expect(w.text()).toContain('+7')
    expect(w.findAll('section.file')).toHaveLength(2)
    expect(w.findAll('section.file')[1]!.attributes('id')).toBe('diff-1')
  })
  it('warns when the server truncated the diff', () => {
    const w = mount(DiffViewer, { props: { files: [file()], truncated: true } })
    expect(w.find('[role="note"]').text()).toContain('cut off')
  })
  it('has an empty state', () => {
    expect(mount(DiffViewer, { props: { files: [] } }).text()).toContain('No differences')
  })
})

const state = (over: Partial<StateInfo> = {}): StateInfo => ({
  key: 'validated',
  name: 'Validated',
  tone: 'blocked',
  value: '0/1 passed',
  summary: 'No trusted passing attestation',
  ...over,
})

describe('StatePill', () => {
  it('conveys the state with an icon, text and a hidden full sentence, not just colour', () => {
    const w = mount(StatePill, { props: { state: state() } })
    expect(w.text()).toContain('Checks')
    expect(w.text()).toContain('0/1 passed')
    expect(w.find('.sr-only').text()).toContain('No trusted passing attestation')
    expect(w.find('[data-state]').attributes('data-tone')).toBe('blocked')
    expect(w.find('svg').exists()).toBe(true)
  })
  it('uses a different glyph per tone', () => {
    const d = (tone: StateInfo['tone']) =>
      mount(StatePill, { props: { state: state({ tone }) } })
        .find('path')
        .attributes('d')
    expect(new Set([d('ok'), d('blocked'), d('failed'), d('pending'), d('na')]).size).toBe(5)
  })
})

describe('LiveIndicator', () => {
  it('shows live and reconnecting distinctly, with a countdown', () => {
    const live = mount(LiveIndicator, { props: { status: 'live' } })
    expect(live.text()).toContain('Live')
    expect(live.attributes('role')).toBe('status')
    expect(live.attributes('data-status')).toBe('live')
    const re = mount(LiveIndicator, { props: { status: 'reconnecting', retryInMs: 4200 } })
    expect(re.text()).toContain('Reconnecting')
    expect(re.text()).toContain('retrying in 5s')
    expect(mount(LiveIndicator, { props: { status: 'unauthenticated' } }).text()).toContain(
      'Signed out',
    )
    expect(mount(LiveIndicator, { props: { status: 'denied' } }).text()).toContain('No access')
    expect(mount(LiveIndicator, { props: { status: 'idle' } }).text()).toContain('Paused')
  })
})
