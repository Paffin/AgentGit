package events

import (
	"context"
	"sync"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil"
)

func wait(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(time.Millisecond)
	}
}

func TestMatchUsesDottedPrefixes(t *testing.T) {
	tests := []struct {
		prefixes []string
		topic    string
		want     bool
	}{
		{nil, "anything", true},
		{[]string{"lease"}, "lease.acquire", true},
		{[]string{"lease"}, "leases.x", false},
		{[]string{"lease"}, "lease", true},
		{[]string{"proposal.create"}, "proposal.create", true},
		{[]string{"proposal.create"}, "proposal.update", false},
		{[]string{"*"}, "x.y", true},
		{[]string{"task", "run"}, "run.finish", true},
	}
	for _, tt := range tests {
		if got := Match(tt.prefixes, tt.topic); got != tt.want {
			t.Errorf("Match(%v,%q) = %v, want %v", tt.prefixes, tt.topic, got, tt.want)
		}
	}
}

func TestTopicsAreTheUnionOrEverything(t *testing.T) {
	h := NewHub(nil, nil, nil)
	h.Subscribe("a", []string{"task", "run"}, func(string, agwp.Event) {})
	h.Subscribe("b", []string{"run", "lease"}, func(string, agwp.Event) {})
	if got := h.Topics(); len(got) != 3 || got[0] != "lease" || got[2] != "task" {
		t.Fatalf("Topics = %v", got)
	}
	h.Subscribe("c", nil, func(string, agwp.Event) {})
	if h.Topics() != nil {
		t.Fatal("a subscriber for everything makes the union unfiltered")
	}
}

func TestFollowDispatchesToMatchingSubscribersAndSurvivesPanics(t *testing.T) {
	w := testutil.NewWorld(t, testutil.WorldOptions{})
	c := w.Orch
	m := metrics.NewService(metrics.NewRegistry())
	h := NewHub(nil, m, nil)
	var mu sync.Mutex
	var tasks, all []string
	h.Subscribe("tasks", []string{"task"}, func(repo string, ev agwp.Event) {
		mu.Lock()
		tasks = append(tasks, ev.Topic)
		mu.Unlock()
		panic("a broken subscriber must not stop the stream")
	})
	h.Subscribe("tasks-too", []string{"task", "lease"}, func(repo string, ev agwp.Event) {
		mu.Lock()
		all = append(all, repo+":"+ev.Topic)
		mu.Unlock()
	})
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	go h.Follow(ctx, c, w.Repo, 0)
	wait(t, "connection", func() bool {
		s := h.Statuses()
		return len(s) == 1 && s[0].Connected
	})
	w.Task("t1", "", nil)
	if _, err := w.Alice.AcquireLease(context.Background(), w.Repo, "docs/**", false, time.Minute, ""); err != nil {
		t.Fatal(err)
	}
	w.Propose("ws-1", "nobody subscribed to proposals", testutil.Put("a.txt", "a\n"))
	wait(t, "events", func() bool { mu.Lock(); defer mu.Unlock(); return len(all) == 2 })
	mu.Lock()
	if len(tasks) != 1 || tasks[0] != "task.create" || all[0] != "acme/web:task.create" || all[1] != "acme/web:lease.acquire" {
		t.Fatalf("tasks=%v all=%v", tasks, all)
	}
	mu.Unlock()
	st := h.Statuses()[0]
	if st.LastSeq == 0 || st.LastEventAt.IsZero() {
		t.Fatalf("status = %+v", st)
	}
	if got := m.EventsReceived.With(w.Repo, "task").Value(); got != 1 {
		t.Fatalf("task events counted = %v", got)
	}
}

func TestReconnectsAreCounted(t *testing.T) {
	w := testutil.NewWorld(t, testutil.WorldOptions{})
	// The proxy forwards to the real server and can cut the event stream, which a healthy server never does.
	px := testutil.NewProxy(t, w.Srv.URL)
	c := w.Srv.ClientAt(px.URL, w.OrchT, "orchestrator")
	m := metrics.NewService(metrics.NewRegistry())
	h := NewHub(nil, m, nil)
	var mu sync.Mutex
	n := 0
	h.Subscribe("x", nil, func(string, agwp.Event) { mu.Lock(); n++; mu.Unlock() })
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	go h.Follow(ctx, c, w.Repo, 0)
	wait(t, "connection", func() bool { return px.OpenEventStreams() == 1 })
	w.Task("before", "", nil)
	wait(t, "first event", func() bool { mu.Lock(); defer mu.Unlock(); return n == 1 })
	px.DropEventStreams()
	w.Task("while-away", "", nil)
	wait(t, "event delivered after the reconnect", func() bool { mu.Lock(); defer mu.Unlock(); return n == 2 })
	if got := m.SSEReconnects.With(w.Repo).Value(); got < 1 {
		t.Fatalf("reconnects counted = %v", got)
	}
	if h.Statuses()[0].Reconnects < 1 {
		t.Fatal("status does not show the reconnect")
	}
}
