// Package loop runs a component's reconcile function when something happened
// (an event "kicked" it) and, as a safety net, on a fixed interval.
//
// Events are only wake-ups: the reconcile function always recomputes from the
// server's current state, so a lost, duplicated or reordered event can delay
// work but never corrupt it.
package loop

import (
	"context"
	"log/slog"
	"sync/atomic"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

// Kicker is a coalescing wake-up signal: many Kick calls before the loop
// wakes up produce one run.
type Kicker struct{ ch chan struct{} }

// NewKicker returns a ready Kicker.
func NewKicker() *Kicker { return &Kicker{ch: make(chan struct{}, 1)} }

// Kick requests a run. It never blocks.
func (k *Kicker) Kick() {
	select {
	case k.ch <- struct{}{}:
	default:
	}
}

// C is the channel the loop waits on.
func (k *Kicker) C() <-chan struct{} { return k.ch }

// Runner drives one reconcile function.
type Runner struct {
	Name     string // component name, a metrics label
	Repo     string
	Interval time.Duration // safety-net period (0 disables)
	Debounce time.Duration // wait this long after a kick so bursts collapse into one run
	Clock    clock.Clock
	Metrics  *metrics.Service
	Log      *slog.Logger

	runs atomic.Uint64
}

// Runs is the number of completed reconcile passes (tests wait on it).
func (r *Runner) Runs() uint64 { return r.runs.Load() }

// Run reconciles once immediately, then whenever kicked or when the interval
// elapses, until ctx is done. A panic in fn is contained and counted as an
// error so one bad proposal cannot take the service down.
func (r *Runner) Run(ctx context.Context, kick *Kicker, fn func(context.Context) error) {
	clk := r.Clock
	if clk == nil {
		clk = clock.Real{}
	}
	log := r.Log
	if log == nil {
		log = slog.Default()
	}
	log = log.With("component", r.Name, "repo", r.Repo)
	pass := func() {
		err := safely(ctx, fn)
		r.runs.Add(1)
		if r.Metrics != nil {
			r.Metrics.ReconcileLoops.With(r.Repo, r.Name).Inc()
			if err != nil && ctx.Err() == nil {
				r.Metrics.ReconcileErrors.With(r.Repo, r.Name).Inc()
			}
		}
		if err != nil && ctx.Err() == nil {
			log.Warn("reconcile failed", "err", err)
		}
	}
	pass()
	var tick clock.Ticker
	var tickC <-chan time.Time
	if r.Interval > 0 {
		tick = clk.NewTicker(r.Interval)
		defer tick.Stop()
		tickC = tick.C()
	}
	for {
		select {
		case <-ctx.Done():
			return
		case <-tickC:
		case <-kick.C():
			if r.Debounce > 0 {
				if err := clock.Sleep(ctx, clk, r.Debounce); err != nil {
					return
				}
			}
		}
		if ctx.Err() != nil {
			return
		}
		pass()
	}
}

func safely(ctx context.Context, fn func(context.Context) error) (err error) {
	defer func() {
		if p := recover(); p != nil {
			err = &PanicError{Value: p}
		}
	}()
	return fn(ctx)
}

// PanicError reports a recovered panic.
type PanicError struct{ Value any }

func (e *PanicError) Error() string { return "panic in reconcile: " + toString(e.Value) }

func toString(v any) string {
	switch x := v.(type) {
	case error:
		return x.Error()
	case string:
		return x
	default:
		return "unknown panic value"
	}
}
