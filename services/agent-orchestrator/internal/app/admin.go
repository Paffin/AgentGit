package app

import (
	"crypto/subtle"
	"encoding/json"
	"net"
	"net/http"
	"sort"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/advisor"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/events"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/execsched"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/review"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/scheduler"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/semindex"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
)

// Status is the document served at GET /status. It contains no credentials.
type Status struct {
	Service  ServiceStatus    `json:"service"`
	Identity []IdentityStatus `json:"identities"`
	Repos    []RepoStatus     `json:"repos"`
	Sessions []session.Info   `json:"sessions"`
}

// ServiceStatus describes the process.
type ServiceStatus struct {
	Version   string    `json:"version"`
	StartedAt time.Time `json:"started_at"`
	Ready     bool      `json:"ready"`
	Reasons   []string  `json:"not_ready_because,omitempty"`
}

// IdentityStatus describes one of the service's credentials (never its secret).
type IdentityStatus struct {
	Label     string    `json:"label"`
	Identity  string    `json:"identity"`
	Roles     []string  `json:"roles"`
	Ops       []string  `json:"operations,omitempty"`
	ExpiresAt time.Time `json:"expires_at,omitempty"`
}

// RepoStatus gathers every component's view of one repository.
type RepoStatus struct {
	Name       string               `json:"name"`
	Stream     string               `json:"stream"`
	Events     *events.FollowStatus `json:"events,omitempty"`
	Scheduler  *scheduler.Status    `json:"scheduler,omitempty"`
	Validation *execsched.Status    `json:"execution_scheduler,omitempty"`
	Advisor    *advisor.Status      `json:"advisor,omitempty"`
	Review     *review.Status       `json:"review,omitempty"`
	Index      *semindex.Status     `json:"index,omitempty"`
}

// Status assembles the current status.
func (a *App) Status() Status {
	ready, reasons := a.readiness()
	st := Status{Service: ServiceStatus{Version: Version, StartedAt: a.started, Ready: ready, Reasons: reasons}, Sessions: a.mgr.Active(),
		Identity: []IdentityStatus{}, Repos: []RepoStatus{}}
	a.mu.Lock()
	for _, label := range []string{"orchestrator", "reviewer", "advisor"} {
		if p := a.principals[label]; p != nil {
			st.Identity = append(st.Identity, IdentityStatus{Label: label, Identity: p.Identity, Roles: p.Roles, Ops: p.Scope.Operations, ExpiresAt: p.ExpiresAt()})
		}
	}
	a.mu.Unlock()
	follow := map[string]events.FollowStatus{}
	for _, f := range a.hub.Statuses() {
		follow[f.Repo] = f
	}
	for _, rt := range a.repos {
		rs := RepoStatus{Name: rt.cfg.Name, Stream: rt.cfg.Stream}
		if f, ok := follow[rt.cfg.Name]; ok {
			rs.Events = &f
		}
		if rt.scheduler != nil {
			s := rt.scheduler.Status()
			rs.Scheduler = &s
		}
		if rt.exec != nil && rt.execShown {
			s := rt.exec.Status()
			rs.Validation = &s
		}
		if rt.advisor != nil {
			s := rt.advisor.Status()
			rs.Advisor = &s
		}
		if rt.review != nil {
			s := rt.review.Status()
			rs.Review = &s
		}
		if rt.index != nil {
			s := rt.index.Status()
			rs.Index = &s
		}
		st.Repos = append(st.Repos, rs)
	}
	if st.Sessions == nil {
		st.Sessions = []session.Info{}
	}
	return st
}

// readiness: the service is ready when it has authenticated to the server and
// every repository's event stream is connected.
func (a *App) readiness() (bool, []string) {
	reasons := []string{} // never null in the JSON
	if !a.preflightOK.Load() {
		a.mu.Lock()
		why := a.preflightEr
		a.mu.Unlock()
		if why == "" {
			why = "has not yet connected to the server"
		}
		reasons = append(reasons, "server: "+why)
	}
	connected := map[string]bool{}
	for _, f := range a.hub.Statuses() {
		connected[f.Repo] = f.Connected
	}
	for _, rt := range a.repos {
		if !connected[rt.cfg.Name] {
			reasons = append(reasons, "event stream of "+rt.cfg.Name+" is not connected")
		}
	}
	sort.Strings(reasons)
	return len(reasons) == 0, reasons
}

func (a *App) listenAddr() string {
	if a.listener != nil {
		return a.listener.Addr().String()
	}
	return a.cfg.Listen
}

// Addr is the address the admin server listens on (valid after Run started it).
func (a *App) Addr() string { return a.listenAddr() }

func (a *App) startHTTP() error {
	ln := a.listener
	if ln == nil {
		var err error
		if ln, err = net.Listen("tcp", a.cfg.Listen); err != nil {
			return err
		}
		a.listener = ln
	}
	a.srv = &http.Server{
		Handler: a.Handler(), ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 15 * time.Second, WriteTimeout: 30 * time.Second,
		IdleTimeout: 60 * time.Second, MaxHeaderBytes: 16 << 10,
	}
	go func() {
		if err := a.srv.Serve(ln); err != nil && err != http.ErrServerClosed {
			a.log.Error("admin server stopped", "err", err)
		}
	}()
	return nil
}

// Handler is the admin and observability HTTP API:
//
//	GET /healthz   liveness (always 200 while the process runs)
//	GET /readyz    200 once connected to the server with all event streams up, else 503
//	GET /metrics   Prometheus text format
//	GET /status    JSON: identities, repositories, sessions, per-proposal four-state view, queues
//	GET /advisor/suggestions?repo=&proposal=   the advisor's stored advice, with suggested files
//	GET /index/…   the semantic index (see package semindex)
//
// /healthz and /readyz are always open (probes); the rest requires the admin
// bearer token when one is configured.
func (a *App) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/plain; charset=utf-8")
		_, _ = w.Write([]byte("ok\n"))
	})
	mux.HandleFunc("/readyz", func(w http.ResponseWriter, r *http.Request) {
		ready, reasons := a.readiness()
		code := http.StatusOK
		if !ready {
			code = http.StatusServiceUnavailable
		}
		writeJSON(w, code, map[string]any{"ready": ready, "not_ready_because": reasons})
	})
	mux.Handle("/metrics", a.auth(a.metrics.Registry.Handler()))
	mux.Handle("/status", a.auth(a.get(func(w http.ResponseWriter, r *http.Request) { writeJSON(w, http.StatusOK, a.Status()) })))
	mux.Handle("/advisor/suggestions", a.auth(a.get(a.suggestions)))
	mux.Handle("/index/", a.auth(a.registry.Handler()))
	return mux
}

func (a *App) get(h http.HandlerFunc) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			writeError(w, http.StatusMethodNotAllowed, agwp.CodeInvalidArgument, "only GET is supported")
			return
		}
		h(w, r)
	})
}

// auth enforces the admin bearer token when one is configured.
func (a *App) auth(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !a.adminTok.Empty() {
			h := r.Header.Get("Authorization")
			tok := strings.TrimPrefix(h, "Bearer ")
			if !strings.HasPrefix(h, "Bearer ") || subtle.ConstantTimeCompare([]byte(tok), []byte(a.adminTok.Reveal())) != 1 {
				w.Header().Set("WWW-Authenticate", `Bearer realm="orchestrator"`)
				writeError(w, http.StatusUnauthorized, agwp.CodeUnauthenticated, "missing or invalid admin token")
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

func (a *App) suggestions(w http.ResponseWriter, r *http.Request) {
	repo, proposal := r.URL.Query().Get("repo"), r.URL.Query().Get("proposal")
	if repo == "" && len(a.repos) == 1 {
		repo = a.repos[0].cfg.Name
	}
	rt := a.byName[repo]
	if rt == nil || rt.advisor == nil {
		writeError(w, http.StatusNotFound, agwp.CodeNotFound, "no advisor for repository "+repo)
		return
	}
	if proposal == "" {
		writeJSON(w, http.StatusOK, rt.advisor.Status())
		return
	}
	rec, ok := rt.advisor.Suggestions(proposal)
	if !ok {
		writeError(w, http.StatusNotFound, agwp.CodeNotFound, "no advice for proposal "+proposal)
		return
	}
	writeJSON(w, http.StatusOK, rec) // []byte fields (suggested files) are base64 in JSON
}

func writeJSON(w http.ResponseWriter, code int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	_ = enc.Encode(v)
}

func writeError(w http.ResponseWriter, code int, errCode, msg string) {
	writeJSON(w, code, map[string]any{"error": map[string]any{"code": errCode, "message": msg}})
}
