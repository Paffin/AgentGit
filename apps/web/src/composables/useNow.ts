import { computed, onScopeDispose, getCurrentScope, ref } from 'vue'
import { serverOffset } from '@/api/clock'

// One shared ticker for every live-updating timestamp on the page. It runs only while something uses it.
const now = ref(Date.now())
let subscribers = 0
let timer: ReturnType<typeof setInterval> | undefined

function acquire() {
  subscribers++
  if (subscribers === 1) {
    now.value = Date.now()
    timer = setInterval(() => {
      now.value = Date.now()
    }, 1000)
  }
}

function release() {
  subscribers = Math.max(0, subscribers - 1)
  if (subscribers === 0 && timer !== undefined) {
    clearInterval(timer)
    timer = undefined
  }
}

/** Local clock, ticking once per second while used. */
export function useNow() {
  if (getCurrentScope()) {
    acquire()
    onScopeDispose(release)
  }
  return now
}

/** Server clock (local clock corrected for measured skew), ticking once per second. */
export function useServerNow() {
  const local = useNow()
  return computed(() => local.value + serverOffset())
}

/** For tests. */
export function __activeTickers(): number {
  return subscribers
}
