// Formatting helpers. All pure: callers pass `now` so tests and live-ticking components agree.

export function shortId(id: string, n = 8): string {
  if (!id) return ''
  // prop_8d079ff9771014b419e9 -> prop_8d079ff9 ; plain hex ids -> first n chars
  const us = id.indexOf('_')
  if (us > 0 && us < 8) return id.slice(0, us + 1 + n)
  return id.slice(0, n)
}

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const

export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '–'
  if (n < 1024) return `${n} B`
  let v = n
  let i = 0
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024
    i++
  }
  return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${UNITS[i]}`
}

export function formatNumber(n: number): string {
  return new Intl.NumberFormat('en-US').format(n)
}

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${formatNumber(n)} ${n === 1 ? one : many}`
}

/** "just now", "12s ago", "5m ago", "3h ago", "2d ago", then a date. Future times read "in 5m". */
export function formatRelative(ms: number, now: number): string {
  if (!ms) return 'never'
  const diff = now - ms
  const abs = Math.abs(diff)
  const future = diff < 0
  if (abs < 5_000) return 'just now'
  const s = Math.round(abs / 1000)
  let text: string
  if (s < 60) text = `${s}s`
  else if (s < 3600) text = `${Math.round(s / 60)}m`
  else if (s < 86_400) text = `${Math.round(s / 3600)}h`
  else if (s < 30 * 86_400) text = `${Math.round(s / 86_400)}d`
  else return formatDate(ms)
  return future ? `in ${text}` : `${text} ago`
}

/** "4m 12s", "1h 05m", "3d 4h". */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const d = Math.floor(total / 86_400)
  const h = Math.floor((total % 86_400) / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (d > 0) return `${d}d ${h}h`
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m`
  if (m > 0) return `${m}m ${String(s).padStart(2, '0')}s`
  return `${s}s`
}

/** Remaining time until `expiresAt`, as a compact countdown; "expired" once it passed. */
export function formatCountdown(expiresAt: number, now: number): string {
  const left = expiresAt - now
  if (left <= 0) return 'expired'
  return formatDuration(left)
}

const pad = (n: number) => String(n).padStart(2, '0')

export function formatDate(ms: number): string {
  const d = new Date(ms)
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

export function formatTime(ms: number): string {
  const d = new Date(ms)
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

export function formatDateTime(ms: number): string {
  if (!ms) return '–'
  return `${formatDate(ms)} ${formatTime(ms).slice(0, 5)}`
}

/** Unambiguous timestamp for tooltips and copy/paste: ISO-8601 in UTC. */
export function formatIso(ms: number): string {
  if (!ms) return ''
  return new Date(ms).toISOString()
}

export function firstLine(text: string): string {
  const i = text.indexOf('\n')
  return i === -1 ? text : text.slice(0, i)
}

/** Tidy "a, b and 3 more". */
export function listSummary(items: readonly string[], max = 3): string {
  if (items.length <= max) return items.join(', ')
  return `${items.slice(0, max).join(', ')} and ${items.length - max} more`
}
