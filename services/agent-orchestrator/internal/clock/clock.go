// Package clock abstracts time so loops, backoff and timeouts can be tested
// without sleeping.
package clock

import (
	"context"
	"time"
)

// Clock is the subset of the time package the service depends on.
type Clock interface {
	// Now returns the current time.
	Now() time.Time
	// NewTimer creates a timer that fires once after d.
	NewTimer(d time.Duration) Timer
	// NewTicker creates a ticker that fires every d (d must be positive).
	NewTicker(d time.Duration) Ticker
}

// Timer is a one-shot timer.
type Timer interface {
	C() <-chan time.Time
	Stop() bool
}

// Ticker is a periodic timer.
type Ticker interface {
	C() <-chan time.Time
	Stop()
}

// Sleep waits for d on c or until ctx is done, whichever comes first.
func Sleep(ctx context.Context, c Clock, d time.Duration) error {
	if d <= 0 {
		return ctx.Err()
	}
	t := c.NewTimer(d)
	defer t.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-t.C():
		return nil
	}
}

// Real is the wall clock.
type Real struct{}

// Now implements Clock.
func (Real) Now() time.Time { return time.Now() }

// NewTimer implements Clock.
func (Real) NewTimer(d time.Duration) Timer { return realTimer{time.NewTimer(d)} }

// NewTicker implements Clock.
func (Real) NewTicker(d time.Duration) Ticker { return realTicker{time.NewTicker(d)} }

type realTimer struct{ t *time.Timer }

func (r realTimer) C() <-chan time.Time { return r.t.C }
func (r realTimer) Stop() bool          { return r.t.Stop() }

type realTicker struct{ t *time.Ticker }

func (r realTicker) C() <-chan time.Time { return r.t.C }
func (r realTicker) Stop()               { r.t.Stop() }
