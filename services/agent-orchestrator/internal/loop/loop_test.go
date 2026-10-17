package loop

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestRunsImmediatelyThenOnTickAndKick(t *testing.T) {
	fake := clock.NewFake(time.Time{})
	var calls atomic.Int32
	r := &Runner{Name: "x", Repo: "a/b", Interval: time.Minute, Clock: fake}
	kick := NewKicker()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	done := make(chan struct{})
	go func() { r.Run(ctx, kick, func(context.Context) error { calls.Add(1); return nil }); close(done) }()
	waitFor(t, "first pass", func() bool { return calls.Load() == 1 })
	waitFor(t, "ticker to be armed", func() bool { return fake.Pending() == 1 })

	fake.Advance(time.Minute)
	waitFor(t, "tick pass", func() bool { return calls.Load() == 2 })

	kick.Kick()
	waitFor(t, "kick pass", func() bool { return calls.Load() == 3 })

	cancel()
	<-done
}

func TestBurstsOfKicksCollapseAfterTheDebounce(t *testing.T) {
	fake := clock.NewFake(time.Time{})
	var calls atomic.Int32
	r := &Runner{Name: "x", Repo: "a/b", Debounce: time.Second, Clock: fake}
	kick := NewKicker()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go r.Run(ctx, kick, func(context.Context) error { calls.Add(1); return nil })
	waitFor(t, "first pass", func() bool { return calls.Load() == 1 })

	for i := 0; i < 50; i++ {
		kick.Kick()
	}
	waitFor(t, "debounce timer", func() bool { return fake.Pending() == 1 })
	if calls.Load() != 1 {
		t.Fatal("ran before the debounce elapsed")
	}
	fake.Advance(time.Second)
	waitFor(t, "second pass", func() bool { return calls.Load() == 2 })
	time.Sleep(20 * time.Millisecond)
	if calls.Load() != 2 {
		t.Fatalf("50 kicks must produce one pass, got %d", calls.Load()-1)
	}
}

func TestErrorsAndPanicsAreContainedAndCounted(t *testing.T) {
	m := metrics.NewService(metrics.NewRegistry())
	fake := clock.NewFake(time.Time{})
	var calls atomic.Int32
	r := &Runner{Name: "sched", Repo: "a/b", Interval: time.Second, Clock: fake, Metrics: m}
	kick := NewKicker()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go r.Run(ctx, kick, func(context.Context) error {
		switch calls.Add(1) {
		case 1:
			return errors.New("boom")
		case 2:
			panic("bad proposal")
		}
		return nil
	})
	waitFor(t, "first pass", func() bool { return r.Runs() == 1 })
	waitFor(t, "ticker", func() bool { return fake.Pending() == 1 })
	fake.Advance(time.Second)
	waitFor(t, "second pass", func() bool { return r.Runs() == 2 })
	waitFor(t, "ticker again", func() bool { return fake.Pending() == 1 })
	fake.Advance(time.Second)
	waitFor(t, "third pass", func() bool { return r.Runs() == 3 })
	if got := m.ReconcileErrors.With("a/b", "sched").Value(); got != 2 {
		t.Fatalf("errors counted = %v, want 2", got)
	}
	if got := m.ReconcileLoops.With("a/b", "sched").Value(); got != 3 {
		t.Fatalf("loops counted = %v, want 3", got)
	}
}

func TestStopsWhenTheContextIsCancelled(t *testing.T) {
	r := &Runner{Name: "x", Repo: "a/b", Interval: time.Hour, Clock: clock.Real{}}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan struct{})
	go func() { r.Run(ctx, NewKicker(), func(context.Context) error { return nil }); close(done) }()
	waitFor(t, "first pass", func() bool { return r.Runs() == 1 })
	cancel()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("Run did not stop")
	}
}

func TestKickerNeverBlocks(t *testing.T) {
	k := NewKicker()
	for i := 0; i < 1000; i++ {
		k.Kick()
	}
	select {
	case <-k.C():
	default:
		t.Fatal("expected a pending kick")
	}
	select {
	case <-k.C():
		t.Fatal("kicks must coalesce into one")
	default:
	}
}
