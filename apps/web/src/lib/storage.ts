// Browser storage can be absent, blocked, full or throw (private windows, disabled site data). Every access
// is wrapped, and the app must work without it.

type Kind = 'session' | 'local'

function area(kind: Kind): Storage | null {
  try {
    return kind === 'session' ? window.sessionStorage : window.localStorage
  } catch {
    return null
  }
}

export function readStore(kind: Kind, key: string): string | null {
  try {
    return area(kind)?.getItem(key) ?? null
  } catch {
    return null
  }
}

export function writeStore(kind: Kind, key: string, value: string): boolean {
  try {
    area(kind)?.setItem(key, value)
    return area(kind) !== null
  } catch {
    return false
  }
}

export function removeStore(kind: Kind, key: string): void {
  try {
    area(kind)?.removeItem(key)
  } catch {
    /* nothing to clean up */
  }
}
