// Package events fans one SSE subscription per repository out to the
// components that want to be woken up by repository events.
package events

import (
	"context"
	"log/slog"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

// Handler receives events. It runs on the stream's goroutine and must return
// quickly (components just kick their reconcile loop).
type Handler func(repo string, ev agwp.Event)

type subscription struct {
	name   string
	topics []string // dotted prefixes; empty = everything
	fn     Handler
}

// FollowStatus describes one repository's stream for /status and /readyz.
type FollowStatus struct {
	Repo        string    `json:"repo"`
	LastSeq     uint64    `json:"last_seq"`
	LastEventAt time.Time `json:"last_event_at,omitempty"`
	Reconnects  uint64    `json:"reconnects"`
	Connected   bool      `json:"connected"`
	LastError   string    `json:"last_error,omitempty"`
}

// Hub owns the subscriptions.
type Hub struct {
	log     *slog.Logger
	metrics *metrics.Service
	clk     clock.Clock

	mu     sync.Mutex
	subs   []subscription
	status map[string]*FollowStatus
}

// NewHub returns an empty hub.
func NewHub(log *slog.Logger, m *metrics.Service, clk clock.Clock) *Hub {
	if log == nil {
		log = slog.Default()
	}
	if clk == nil {
		clk = clock.Real{}
	}
	return &Hub{log: log, metrics: m, clk: clk, status: map[string]*FollowStatus{}}
}

// Subscribe registers fn for events whose topic equals or is below one of the
// dotted prefixes (none = all topics). Subscribe before Follow.
func (h *Hub) Subscribe(name string, topics []string, fn Handler) {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.subs = append(h.subs, subscription{name: name, topics: topics, fn: fn})
}

// Topics is the union of every subscriber's topics, or nil when any wants everything.
func (h *Hub) Topics() []string {
	h.mu.Lock()
	defer h.mu.Unlock()
	set := map[string]bool{}
	for _, s := range h.subs {
		if len(s.topics) == 0 {
			return nil
		}
		for _, t := range s.topics {
			set[t] = true
		}
	}
	out := make([]string, 0, len(set))
	for t := range set {
		out = append(out, t)
	}
	sort.Strings(out)
	return out
}

// Match reports whether topic equals prefix or lies below it ("lease" matches "lease.acquire").
func Match(prefixes []string, topic string) bool {
	if len(prefixes) == 0 {
		return true
	}
	for _, p := range prefixes {
		if p == "*" || topic == p || strings.HasPrefix(topic, p+".") {
			return true
		}
	}
	return false
}

// Follow streams the repository's events with client until ctx is done.
func (h *Hub) Follow(ctx context.Context, client *agwp.Client, repo string, idle time.Duration) {
	h.mu.Lock()
	st := &FollowStatus{Repo: repo}
	h.status[repo] = st
	h.mu.Unlock()

	f := client.NewFollower(agwp.FollowOptions{
		Repo:        repo,
		Topics:      h.Topics(),
		IdleTimeout: idle,
		Logger:      h.log,
		OnConnect: func() {
			h.mu.Lock()
			st.Connected = true
			h.mu.Unlock()
		},
		OnReconnect: func(err error) {
			if h.metrics != nil {
				h.metrics.SSEReconnects.With(repo).Inc()
			}
			h.mu.Lock()
			st.Reconnects++
			st.Connected = false
			if err != nil {
				st.LastError = err.Error()
			}
			h.mu.Unlock()
		},
	})
	_ = f.Run(ctx, func(ev agwp.Event) {
		h.mu.Lock()
		st.LastSeq, st.LastEventAt = ev.Seq, h.clk.Now()
		subs := append([]subscription(nil), h.subs...)
		h.mu.Unlock()
		if h.metrics != nil {
			h.metrics.EventsReceived.With(repo, topLevel(ev.Topic)).Inc()
		}
		for _, s := range subs {
			if Match(s.topics, ev.Topic) {
				func() {
					defer func() {
						if p := recover(); p != nil {
							h.log.Error("event handler panicked", "subscriber", s.name, "topic", ev.Topic, "panic", p)
						}
					}()
					s.fn(repo, ev)
				}()
			}
		}
	})
}

// topLevel keeps metric cardinality bounded: "lease.acquire" -> "lease".
func topLevel(topic string) string {
	if i := strings.IndexByte(topic, '.'); i > 0 {
		return topic[:i]
	}
	return topic
}

// Statuses returns a snapshot of every followed repository, sorted by name.
func (h *Hub) Statuses() []FollowStatus {
	h.mu.Lock()
	defer h.mu.Unlock()
	out := make([]FollowStatus, 0, len(h.status))
	for _, s := range h.status {
		out = append(out, *s)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Repo < out[j].Repo })
	return out
}
