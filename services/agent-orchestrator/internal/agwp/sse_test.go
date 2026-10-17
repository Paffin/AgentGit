package agwp

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestReadSSEParsesFieldsCommentsAndMultilineData(t *testing.T) {
	stream := ": heartbeat\n\nid: 5\nevent: task.create\ndata: {\"a\":\ndata: 1}\n\n:another comment\nid: 6\ndata: x\n\n"
	var got []sseEvent
	touches := 0
	err := readSSE(strings.NewReader(stream), func() { touches++ }, func(e sseEvent) error {
		got = append(got, e)
		return nil
	})
	if err != errStreamClosed {
		t.Fatalf("err = %v", err)
	}
	if len(got) != 2 || got[0].id != "5" || got[0].name != "task.create" || got[0].data != "{\"a\":\n1}" || got[1].data != "x" {
		t.Fatalf("events = %+v", got)
	}
	if touches == 0 {
		t.Fatal("touch was never called")
	}
}

func TestReadSSEPropagatesHandlerErrors(t *testing.T) {
	boom := fmt.Errorf("boom")
	err := readSSE(strings.NewReader("data: x\n\n"), func() {}, func(sseEvent) error { return boom })
	if err != boom {
		t.Fatalf("err = %v", err)
	}
}

func TestReadSSERejectsRunawayLines(t *testing.T) {
	err := readSSE(strings.NewReader("data: "+strings.Repeat("x", maxSSELine+10)+"\n\n"), func() {}, func(sseEvent) error { return nil })
	if err == nil || !strings.Contains(err.Error(), "longer than") {
		t.Fatalf("err = %v", err)
	}
}

// sseServer scripts successive connections.
type sseServer struct {
	mu       sync.Mutex
	conns    []sseConn // what each connection saw
	scripts  []func(w http.ResponseWriter, r *http.Request)
	latest   uint64
	replays  int
	finished chan struct{}
}

type sseConn struct {
	afterSeq, lastEventID, heartbeat string
}

func (s *sseServer) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case strings.HasSuffix(r.URL.Path, "/events/replay"):
			s.mu.Lock()
			s.replays++
			l := s.latest
			s.mu.Unlock()
			_, _ = fmt.Fprintf(w, `{"events":[],"latest_seq":%d}`, l)
		case strings.HasSuffix(r.URL.Path, "/events"):
			s.mu.Lock()
			idx := len(s.conns)
			s.conns = append(s.conns, sseConn{r.URL.Query().Get("after_seq"), r.Header.Get("Last-Event-ID"), r.URL.Query().Get("heartbeat_ms")})
			var script func(http.ResponseWriter, *http.Request)
			if idx < len(s.scripts) {
				script = s.scripts[idx]
			}
			s.mu.Unlock()
			w.Header().Set("Content-Type", "text/event-stream")
			if script == nil {
				<-r.Context().Done()
				return
			}
			script(w, r)
		}
	})
}

func (s *sseServer) connections() []sseConn {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]sseConn(nil), s.conns...)
}

func emit(w http.ResponseWriter, seq int, topic string) {
	fmt.Fprintf(w, "id: %d\nevent: %s\ndata: {\"seq\":%d,\"topic\":%q,\"repo\":\"acme/web\",\"payload\":{\"actor\":\"user:a\",\"target\":\"t%d\",\"details\":{\"k\":\"v%d\"}}}\n\n", seq, topic, seq, topic, seq, seq)
	w.(http.Flusher).Flush()
}

func TestFollowerStartsLiveAndResumesAfterDrops(t *testing.T) {
	srv := &sseServer{latest: 40}
	srv.scripts = []func(http.ResponseWriter, *http.Request){
		func(w http.ResponseWriter, r *http.Request) { // delivers 41, 42 then drops
			emit(w, 41, "task.create")
			emit(w, 42, "task.finish")
		},
		func(w http.ResponseWriter, r *http.Request) { // replays 42 again (overlap), then 43, then an error event
			emit(w, 42, "task.finish")
			emit(w, 43, "run.finish")
			fmt.Fprint(w, "event: error\ndata: {\"error\":{\"code\":\"unauthenticated\",\"message\":\"the credential expired\"}}\n\n")
		},
	}
	c, ck, obs := newTestClient(t, srv.Handler())
	var mu sync.Mutex
	var got []Event
	done := make(chan struct{})
	reconnects := make(chan error, 8)
	f := c.NewFollower(FollowOptions{Repo: "acme/web", Topics: []string{"task", "run"}, OnReconnect: func(err error) { reconnects <- err }})
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel) // runs before the test server closes, so no SSE handler is left waiting
	go func() {
		_ = f.Run(ctx, func(e Event) {
			mu.Lock()
			got = append(got, e)
			n := len(got)
			mu.Unlock()
			if n == 3 {
				close(done)
			}
		})
	}()
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatalf("timed out; got %+v", got)
	}
	// The third connection is the reconnect after the error event.
	deadline := time.Now().Add(5 * time.Second)
	for len(srv.connections()) < 3 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	cancel()
	mu.Lock()
	defer mu.Unlock()
	if len(got) != 3 || got[0].Seq != 41 || got[1].Seq != 42 || got[2].Seq != 43 {
		t.Fatalf("events = %+v (the overlapping replay of 42 must be dropped)", got)
	}
	if got[2].Topic != "run.finish" || got[0].Detail("k") != "v41" || got[0].Payload.Actor != "user:a" {
		t.Fatalf("event decoding wrong: %+v", got[0])
	}
	conns := srv.connections()
	if len(conns) < 3 {
		t.Fatalf("connections = %+v", conns)
	}
	if conns[0].afterSeq != "40" || conns[0].lastEventID != "40" || conns[0].heartbeat != "15000" {
		t.Errorf("first connection must start live from the log position: %+v", conns[0])
	}
	if conns[1].afterSeq != "42" || conns[1].lastEventID != "42" {
		t.Errorf("reconnect must resume after the last handled event: %+v", conns[1])
	}
	if conns[2].afterSeq != "43" {
		t.Errorf("reconnect after the error event must resume at 43: %+v", conns[2])
	}
	if srv.replays != 1 {
		t.Errorf("the log position is read once, got %d", srv.replays)
	}
	if len(obs.errors) == 0 || obs.errors[len(obs.errors)-1] != CodeUnauthenticated {
		t.Errorf("the error event must be observed: %v", obs.errors)
	}
	// the error event reached OnReconnect as a typed error
	var sawTyped bool
	for len(reconnects) > 0 {
		if IsCode(<-reconnects, CodeUnauthenticated) {
			sawTyped = true
		}
	}
	if !sawTyped {
		t.Error("OnReconnect never saw the typed unauthenticated error")
	}
	if len(ck.sleeps()) == 0 {
		t.Error("reconnects must back off")
	}
}

func TestFollowerBackoffGrowsThenResetsAfterProgress(t *testing.T) {
	srv := &sseServer{}
	// three connections that fail at once, then one that delivers an event, then another failure
	fail := func(w http.ResponseWriter, r *http.Request) {}
	srv.scripts = []func(http.ResponseWriter, *http.Request){fail, fail, fail,
		func(w http.ResponseWriter, r *http.Request) { emit(w, 1, "task.create") },
		fail,
	}
	c, ck, _ := newTestClient(t, srv.Handler())
	f := c.NewFollower(FollowOptions{Repo: "acme/web", FromStart: true, BackoffBase: time.Second, BackoffMax: time.Minute})
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	got := make(chan struct{}, 1)
	go func() { _ = f.Run(ctx, func(Event) { got <- struct{}{} }) }()
	select {
	case <-got:
	case <-time.After(5 * time.Second):
		t.Fatal("no event")
	}
	deadline := time.Now().Add(5 * time.Second)
	for len(srv.connections()) < 6 && time.Now().Before(deadline) {
		time.Sleep(time.Millisecond)
	}
	cancel()
	s := ck.sleeps()
	if len(s) < 5 {
		t.Fatalf("sleeps = %v", s)
	}
	within := func(d, base time.Duration) bool { return d >= base*8/10 && d <= base*12/10 }
	if !within(s[0], time.Second) || !within(s[1], 2*time.Second) || !within(s[2], 4*time.Second) {
		t.Fatalf("backoff must double: %v", s[:3])
	}
	if !within(s[3], time.Second) {
		t.Fatalf("backoff must reset after an event was received: %v", s[:5])
	}
}

func TestFollowerReconnectsWhenTheServerGoesSilent(t *testing.T) {
	srv := &sseServer{}
	srv.scripts = []func(http.ResponseWriter, *http.Request){
		func(w http.ResponseWriter, r *http.Request) {
			emit(w, 1, "task.create")
			<-r.Context().Done() // then nothing, not even heartbeats
		},
	}
	c, _, _ := newTestClient(t, srv.Handler())
	f := c.NewFollower(FollowOptions{Repo: "acme/web", FromStart: true, IdleTimeout: 80 * time.Millisecond})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go func() { _ = f.Run(ctx, func(Event) {}) }()
	deadline := time.Now().Add(5 * time.Second)
	for len(srv.connections()) < 2 {
		if time.Now().After(deadline) {
			t.Fatal("the follower never gave up on a silent stream")
		}
		time.Sleep(5 * time.Millisecond)
	}
	if got := srv.connections()[1].afterSeq; got != "1" {
		t.Fatalf("reconnect after_seq = %s", got)
	}
}

func TestFollowerSurvivesAuthFailuresOnConnect(t *testing.T) {
	var mu sync.Mutex
	calls := 0
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		calls++
		n := calls
		mu.Unlock()
		if n <= 2 {
			apiError(w, 401, CodeUnauthenticated, false, 0)
			return
		}
		w.Header().Set("Content-Type", "text/event-stream")
		emit(w, 1, "task.create")
		<-r.Context().Done()
	}))
	f := c.NewFollower(FollowOptions{Repo: "acme/web", FromStart: true})
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	got := make(chan Event, 1)
	var errs []error
	var emu sync.Mutex
	f.opt.OnReconnect = func(err error) { emu.Lock(); errs = append(errs, err); emu.Unlock() }
	go func() { _ = f.Run(ctx, func(e Event) { got <- e }) }()
	select {
	case e := <-got:
		if e.Seq != 1 {
			t.Fatalf("event = %+v", e)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("never recovered")
	}
	emu.Lock()
	defer emu.Unlock()
	if len(errs) != 2 || !IsCode(errs[0], CodeUnauthenticated) {
		t.Fatalf("reconnect errors = %v", errs)
	}
}

func TestFollowerStopsOnContextCancel(t *testing.T) {
	srv := &sseServer{latest: 3}
	c, _, _ := newTestClient(t, srv.Handler())
	f := c.NewFollower(FollowOptions{Repo: "acme/web"})
	ctx, cancel := context.WithCancel(context.Background())
	errc := make(chan error, 1)
	go func() { errc <- f.Run(ctx, func(Event) {}) }()
	for len(srv.connections()) == 0 {
		time.Sleep(time.Millisecond)
	}
	cancel()
	select {
	case err := <-errc:
		if err != context.Canceled {
			t.Fatalf("err = %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("Run did not return after cancel")
	}
	if f.LastSeq() != 3 {
		t.Fatalf("LastSeq = %d", f.LastSeq())
	}
}

func TestReplayEventsBuildsTheQuery(t *testing.T) {
	var q string
	c, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		q = r.URL.RawQuery
		_, _ = io.WriteString(w, `{"events":[{"seq":8,"topic":"lease.acquire"}],"latest_seq":9}`)
	}))
	evs, latest, err := c.ReplayEvents(context.Background(), "acme/web", 7, 50, []string{"lease", "run"})
	if err != nil || latest != 9 || len(evs) != 1 || evs[0].Seq != 8 {
		t.Fatalf("evs=%+v latest=%d err=%v", evs, latest, err)
	}
	if q != "after_seq=7&limit=50&topics=lease%2Crun" {
		t.Fatalf("query = %s", q)
	}
}
