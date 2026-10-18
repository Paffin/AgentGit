// Package fakeclock is a manually advanced clock.Clock for tests of timers, tickers, backoff and timeouts. It lives
// under testutil so that no test clock ships in the product packages.
package fakeclock

import (
	"sort"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
)

// Fake is a manually advanced clock for tests (it implements clock.Clock). It is safe for concurrent use.
type Fake struct {
	mu     sync.Mutex
	now    time.Time
	timers []*fakeTimer
}

// New returns a fake clock starting at start (the Unix epoch plus one day
// when start is the zero time, so durations before "now" stay positive).
func New(start time.Time) *Fake {
	if start.IsZero() {
		start = time.Unix(86400, 0).UTC()
	}
	return &Fake{now: start}
}

// Now implements Clock.
func (f *Fake) Now() time.Time {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.now
}

type fakeTimer struct {
	f      *Fake
	when   time.Time
	period time.Duration // 0 for one-shot
	ch     chan time.Time
	active bool
}

func (t *fakeTimer) C() <-chan time.Time { return t.ch }

func (t *fakeTimer) Stop() bool {
	t.f.mu.Lock()
	defer t.f.mu.Unlock()
	was := t.active
	t.active = false
	t.f.removeLocked(t)
	return was
}

type fakeTicker struct{ *fakeTimer }

func (t fakeTicker) Stop() { t.fakeTimer.Stop() }

func (f *Fake) removeLocked(t *fakeTimer) {
	for i, x := range f.timers {
		if x == t {
			f.timers = append(f.timers[:i], f.timers[i+1:]...)
			return
		}
	}
}

// NewTimer implements Clock.
func (f *Fake) NewTimer(d time.Duration) clock.Timer {
	f.mu.Lock()
	defer f.mu.Unlock()
	t := &fakeTimer{f: f, when: f.now.Add(d), ch: make(chan time.Time, 1), active: true}
	if d <= 0 {
		t.ch <- f.now
		t.active = false
		return t
	}
	f.timers = append(f.timers, t)
	return t
}

// NewTicker implements Clock.
func (f *Fake) NewTicker(d time.Duration) clock.Ticker {
	if d <= 0 {
		panic("fakeclock: non-positive ticker interval")
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	t := &fakeTimer{f: f, when: f.now.Add(d), period: d, ch: make(chan time.Time, 1), active: true}
	f.timers = append(f.timers, t)
	return fakeTicker{t}
}

// Advance moves the clock forward by d, firing every timer and ticker that
// becomes due, in order.
func (f *Fake) Advance(d time.Duration) {
	f.mu.Lock()
	target := f.now.Add(d)
	for {
		sort.SliceStable(f.timers, func(i, j int) bool { return f.timers[i].when.Before(f.timers[j].when) })
		if len(f.timers) == 0 || f.timers[0].when.After(target) {
			break
		}
		t := f.timers[0]
		f.now = t.when
		select {
		case t.ch <- t.when:
		default: // a slow receiver drops ticks, like time.Ticker
		}
		if t.period > 0 {
			t.when = t.when.Add(t.period)
		} else {
			t.active = false
			f.timers = f.timers[1:]
		}
	}
	f.now = target
	f.mu.Unlock()
}

// Pending reports how many timers and tickers are waiting (useful to wait
// until a goroutine has parked on the clock before advancing it).
func (f *Fake) Pending() int {
	f.mu.Lock()
	defer f.mu.Unlock()
	return len(f.timers)
}
