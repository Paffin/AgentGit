// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { filterAttention } from './attention-filter'
import type { AttentionItem } from './attention'

const items: AttentionItem[] = [
  {
    key: 'one',
    kind: 'review',
    repo: 'acme/web',
    title: 'Handle Unicode',
    reason: 'Needs approval',
    who: 'alice',
    byAgent: false,
    more: 0,
    ageMs: 1,
  },
  {
    key: 'two',
    kind: 'blocked',
    repo: 'acme/api',
    title: 'Retry safely',
    reason: 'Missing tests',
    who: 'build-bot',
    byAgent: true,
    more: 0,
    ageMs: 2,
  },
]
describe('attention filters', () => {
  it('matches words across title, author, reason and repository without case sensitivity', () => {
    expect(filterAttention(items, { query: '  ACME/api BUILD-bot ' }).map((i) => i.key)).toEqual([
      'two',
    ])
    expect(filterAttention(items, { query: 'approval Unicode' }).map((i) => i.key)).toEqual(['one'])
  })
  it('intersects exact repository and category filters', () => {
    expect(filterAttention(items, { repo: 'acme/web', kind: 'blocked' })).toEqual([])
    expect(filterAttention(items, { repo: 'acme/web', kind: 'review' })).toEqual([items[0]])
    expect(filterAttention(items, { repo: 'acme/we' })).toEqual([])
  })
  it('keeps every item for empty filters and never mutates the input', () => {
    expect(filterAttention(items, {})).toEqual(items)
    expect(filterAttention(items, { query: 'missing', kind: 'review' })).toEqual([])
    expect(items.map((i) => i.key)).toEqual(['one', 'two'])
  })
})
