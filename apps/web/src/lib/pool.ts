/** Runs `fn` over `items` with at most `limit` in flight; results keep the input order. Never rejects. */
export async function mapLimit<T, R>(
  items: readonly T[],
  limit: number,
  fn: (item: T, index: number) => Promise<R>,
): Promise<PromiseSettledResult<R>[]> {
  const results: PromiseSettledResult<R>[] = new Array(items.length)
  let next = 0
  async function worker() {
    for (;;) {
      const i = next++
      if (i >= items.length) return
      try {
        results[i] = { status: 'fulfilled', value: await fn(items[i] as T, i) }
      } catch (reason) {
        results[i] = { status: 'rejected', reason }
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker))
  return results
}

/** Coalesces bursts: `fn` runs once, `delay` ms after the last call; calls during a run schedule another run. */
export function coalesce(fn: () => Promise<void> | void, delay: number) {
  let timer: ReturnType<typeof setTimeout> | undefined
  let running = false
  let again = false
  const run = async () => {
    timer = undefined
    if (running) {
      again = true
      return
    }
    running = true
    try {
      await fn()
    } finally {
      running = false
      if (again) {
        again = false
        timer = setTimeout(run, delay)
      }
    }
  }
  return {
    trigger() {
      if (running) {
        again = true
        return
      }
      clearTimeout(timer)
      timer = setTimeout(run, delay)
    },
    cancel() {
      clearTimeout(timer)
      timer = undefined
      again = false
    },
  }
}
