package fakeclock

import (
	"testing"
	"time"
)

func TestFakeTimerFiresOnAdvance(t *testing.T) {
	f := New(time.Time{})
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
	f := New(time.Time{})
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
	f := New(time.Time{})
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
	f := New(time.Time{})
	tm := f.NewTimer(0)
	select {
	case <-tm.C():
	default:
		t.Fatal("zero timer must fire immediately")
	}
}
