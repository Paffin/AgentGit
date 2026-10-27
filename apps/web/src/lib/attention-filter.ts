import type { AttentionItem, AttentionKind } from './attention'

export interface AttentionFilter {
  query?: string
  repo?: string
  kind?: AttentionKind
}

/** Each search word must occur somewhere in the item's visible text. */
function matchesQuery(item: AttentionItem, words: readonly string[]): boolean {
  const text = [item.title, item.reason, item.who, item.repo].join(' ').toLocaleLowerCase()
  return words.every((word) => text.includes(word))
}

export function filterAttention(
  items: readonly AttentionItem[],
  filter: AttentionFilter,
): AttentionItem[] {
  const words = (filter.query ?? '').trim().toLocaleLowerCase().split(/\s+/).filter(Boolean)
  return items.filter(
    (item) =>
      (!filter.repo || item.repo === filter.repo) &&
      (!filter.kind || item.kind === filter.kind) &&
      matchesQuery(item, words),
  )
}
