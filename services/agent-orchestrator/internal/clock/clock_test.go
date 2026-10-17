package clock

import (
	"context"
	"testing"
	"time"
)

func TestFakeTimerFiresOnAdvance(t *testing.T) {
	f := NewFake(time.Time{})
	start := f.Now()
	tm := f.NewTimer(10 * time.Second)
	select {
	case <-tm.C():
		t.Fatal("timer fired early")
	default:
	}
	f.Advance(9 * time.Second)
	select {
	case <-tm.C():
		t.Fatal("timer fired early")
	default:
	}
	f.Advance(time.Second)
	select {
	case got := <-tm.C():
		if !got.Equal(start.Add(10 * time.Second)) {
			t.Fatalf("fired at %v", got)
		}
	default:
		t.Fatal("timer did not fire")
	}
	if f.Pending() != 0 {
		t.Fatalf("pending = %d", f.Pending())
	}
}

func TestFakeTimerStop(t *testing.T) {
	f := NewFake(time.Time{})
	tm := f.NewTimer(time.Second)
	if !tm.Stop() {
		t.Fatal("Stop on an active timer must report true")
	}
	if tm.Stop() {
		t.Fatal("second Stop must report false")
	}
	f.Advance(time.Hour)
	select {
	case <-tm.C():
		t.Fatal("stopped timer fired")
	default:
	}
}

func TestFakeTickerKeepsTicking(t *testing.T) {
	f := NewFake(time.Time{})
	tk := f.NewTicker(time.Second)
	defer tk.Stop()
	n := 0
	for i := 0; i < 3; i++ {
		f.Advance(time.Second)
		select {
		case <-tk.C():
			n++
		default:
		}
	}
	if n != 3 {
		t.Fatalf("ticks = %d, want 3", n)
	}
	tk.Stop()
	f.Advance(time.Hour)
	select {
	case <-tk.C():
		t.Fatal("stopped ticker fired")
	default:
	}
}

func TestFakeNonPositiveTimerFiresImmediately(t *testing.T) {
	f := NewFake(time.Time{})
	tm := f.NewTimer(0)
	select {
	case <-tm.C():
	default:
		t.Fatal("zero timer must fire immediately")
	}
}

func TestSleepHonoursContext(t *testing.T) {
	f := NewFake(time.Time{})
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- Sleep(ctx, f, time.Hour) }()
	for f.Pending() == 0 {
		time.Sleep(time.Millisecond)
	}
	cancel()
	if err := <-done; err == nil {
		t.Fatal("expected context error")
	}
}

func TestSleepReturnsAfterAdvance(t *testing.T) {
	f := NewFake(time.Time{})
	done := make(chan error, 1)
	go func() { done <- Sleep(context.Background(), f, time.Minute) }()
	for f.Pending() == 0 {
		time.Sleep(time.Millisecond)
	}
	f.Advance(time.Minute)
	if err := <-done; err != nil {
		t.Fatal(err)
	}
}

func TestRealClockBasics(t *testing.T) {
	var c Clock = Real{}
	if c.Now().IsZero() {
		t.Fatal("zero time")
	}
	tm := c.NewTimer(time.Millisecond)
	<-tm.C()
	tk := c.NewTicker(time.Millisecond)
	<-tk.C()
	tk.Stop()
}
