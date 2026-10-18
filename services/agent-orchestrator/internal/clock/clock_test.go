package clock_test

import (
	"context"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeclock"
)

func TestSleepHonoursContext(t *testing.T) {
	f := fakeclock.New(time.Time{})
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- clock.Sleep(ctx, f, time.Hour) }()
	for f.Pending() == 0 {
		time.Sleep(time.Millisecond)
	}
	cancel()
	if err := <-done; err == nil {
		t.Fatal("expected context error")
	}
}

func TestSleepReturnsAfterAdvance(t *testing.T) {
	f := fakeclock.New(time.Time{})
	done := make(chan error, 1)
	go func() { done <- clock.Sleep(context.Background(), f, time.Minute) }()
	for f.Pending() == 0 {
		time.Sleep(time.Millisecond)
	}
	f.Advance(time.Minute)
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestRealClockBasics(t *testing.T) {
	var c clock.Clock = clock.Real{}
	if c.Now().IsZero() {
		t.Fatal("zero time")
	}
	tm := c.NewTimer(time.Millisecond)
	<-tm.C()
	tk := c.NewTicker(time.Millisecond)
	<-tk.C()
	tk.Stop()
}
