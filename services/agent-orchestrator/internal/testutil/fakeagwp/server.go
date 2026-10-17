// Package fakeagwp is an in-memory AGWP v1 HTTP/JSON server for unit tests.
//
// It mimics the real server where the orchestrator depends on it: the error
// model ({"error": {...}} with stable codes, Retry-After), bearer credentials
// with roles and narrowing scopes, agent runs acting through the ag-run header,
// task ownership and dependencies, leases with fencing tokens, the audit-backed
// event feed (replay and SSE with resume), and proposals with reviews. It does
// NOT merge anything: tests script preparations and file contents directly.
//
// The integration suite (internal/integration) runs against the real server;
// this fake exists so the packages can be tested in isolation and so failure
// injection (rate limits, dropped streams) is deterministic.
package fakeagwp

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

// Server is the fake. Create with New; always Close it.
type Server struct {
	URL string

	mu      sync.Mutex
	srv     *httptest.Server
	now     func() time.Time
	tokens  map[string]*tokenInfo // by secret
	creds   map[string]*tokenInfo // by credential id
	repos   map[string]*repoState
	fails   map[string][]failRule
	reqLog  []RequestRecord
	seq     int
	streams int // currently open SSE connections
	sseLog  []SSEConnect
	kill    chan struct{} // closed to drop all SSE connections
	// StubJobs makes prepare(enqueue_validations) answer with one queued job per
	// missing check, instead of the real server's empty list.
	StubJobs bool
}

// RequestRecord is one request the server saw.
type RequestRecord struct {
	Method, Path, Query string
	Identity, Run       string
	Header              http.Header
	Body                string
	Op                  string
}

// SSEConnect records how a client (re)connected to the event stream.
type SSEConnect struct {
	Repo        string
	AfterSeq    string
	LastEventID string
	Topics      string
}

type failRule struct {
	status       int
	code         string
	message      string
	retryable    bool
	retryAfterMs int64
	times        int // remaining; <0 = forever
	closeConn    bool
}

type tokenInfo struct {
	id        string
	secret    string
	identity  string
	role      string // reader | writer | reviewer | integrator | admin
	repos     []string
	ops       map[string]bool
	streams   []string
	expiresAt time.Time
	revoked   bool
	parent    string
}

type repoState struct {
	name       string
	streams    map[string]*streamState
	tasks      []*taskState
	agents     map[string]*agwp.Agent
	runs       []*agwp.Run
	leases     []*agwp.Lease
	fence      uint64
	workspaces map[string]*wsState
	changesets map[string]*csState
	proposals  []*agwp.Proposal
	reviews    map[string][]agwp.Review
	reviewSeq  int64
	prepares   map[string]*agwp.Preparation
	policy     map[string]Policy
	approvals  map[string]map[string]bool // proposal+head → approver set
	attested   map[string]map[string]bool // tree id → passed checks
	events     []agwp.Event
	notify     chan struct{}
	predict    []agwp.Overlap
	provenance []agwp.ProvenanceEntry
	jobs       []agwp.Job
	queue      []agwp.QueueEntry
}

type streamState struct {
	head    string
	version uint64
}

type taskState struct {
	agwp.Task
}

type wsState struct {
	agwp.Workspace
}

type csState struct {
	id      string
	parents []string
	files   map[string][]byte
	author  string
	message string
	ts      int64
}

// Policy is the tiny slice of stream policy the fake evaluates in prepare.
type Policy struct {
	RequiredChecks []string
	MinApprovals   int
}

// New starts a server.
func New() *Server {
	s := &Server{
		now:    time.Now,
		tokens: map[string]*tokenInfo{},
		creds:  map[string]*tokenInfo{},
		repos:  map[string]*repoState{},
		fails:  map[string][]failRule{},
		kill:   make(chan struct{}),
	}
	s.srv = httptest.NewServer(http.HandlerFunc(s.serve))
	s.URL = s.srv.URL
	return s
}

// Close shuts the server down.
func (s *Server) Close() {
	s.mu.Lock()
	select {
	case <-s.kill:
	default:
		close(s.kill)
	}
	s.mu.Unlock()
	s.srv.CloseClientConnections()
	s.srv.Close()
}

// SetClock overrides the time source (lease expiry, timestamps).
func (s *Server) SetClock(f func() time.Time) {
	s.mu.Lock()
	s.now = f
	s.mu.Unlock()
}

func (s *Server) nowMs() int64 { return s.now().UnixMilli() }

// ---- test fixtures --------------------------------------------------------------------

// AddRepo creates a repository with an empty stream "main".
func (s *Server) AddRepo(name string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.repos[name] = &repoState{
		name:       name,
		streams:    map[string]*streamState{"main": {}},
		agents:     map[string]*agwp.Agent{},
		workspaces: map[string]*wsState{},
		changesets: map[string]*csState{},
		reviews:    map[string][]agwp.Review{},
		prepares:   map[string]*agwp.Preparation{},
		policy:     map[string]Policy{},
		approvals:  map[string]map[string]bool{},
		attested:   map[string]map[string]bool{},
		notify:     make(chan struct{}),
	}
	s.emitLocked(s.repos[name], "repo.init", "user:root", name, map[string]any{"default_stream": "main"})
}

// TokenSpec describes a credential to add.
type TokenSpec struct {
	Identity string
	Role     string   // default "writer"
	Repos    []string // scope; empty = any
	Ops      []string // scope; empty = any
	Streams  []string
	TTL      time.Duration // 0 = never expires
}

// AddToken registers a credential and returns its secret.
func (s *Server) AddToken(spec TokenSpec) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.addTokenLocked(spec, "").secret
}

func (s *Server) addTokenLocked(spec TokenSpec, parent string) *tokenInfo {
	if spec.Role == "" {
		spec.Role = "writer"
	}
	id := "cred_" + randHex(8)
	t := &tokenInfo{id: id, secret: "agt_" + id + "_" + randHex(16), identity: spec.Identity, role: spec.Role,
		repos: spec.Repos, ops: map[string]bool{}, streams: spec.Streams, parent: parent}
	for _, o := range spec.Ops {
		t.ops[o] = true
	}
	if spec.TTL > 0 {
		t.expiresAt = s.now().Add(spec.TTL)
	}
	s.tokens[t.secret] = t
	s.creds[id] = t
	return t
}

// Revoked reports whether the credential with this secret has been revoked.
func (s *Server) Revoked(secret string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.tokens[secret]
	return t != nil && t.revoked
}

// MintedTokens returns every credential minted through the API (identity, ops, ttl left, revoked).
func (s *Server) MintedTokens() []MintedInfo {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []MintedInfo
	for _, t := range s.creds {
		if t.parent != "" {
			out = append(out, MintedInfo{ID: t.id, Secret: t.secret, Identity: t.identity, Repos: t.repos,
				Ops: sortedKeys(t.ops), Streams: t.streams, ExpiresAt: t.expiresAt, Revoked: t.revoked})
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// MintedInfo describes a credential minted through POST /v1/credentials.
type MintedInfo struct {
	ID, Secret, Identity string
	Repos, Ops, Streams  []string
	ExpiresAt            time.Time
	Revoked              bool
}

// FailNext makes the next n requests for operation op fail with the given
// HTTP status and AGWP error code (n < 0: forever). op is the operation id,
// e.g. "task.list", or "*" for every request.
func (s *Server) FailNext(op string, n int, status int, code string, retryable bool, retryAfterMs int64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.fails[op] = append(s.fails[op], failRule{status: status, code: code, message: "injected failure", retryable: retryable, retryAfterMs: retryAfterMs, times: n})
}

// DropConnectionNext makes the next n requests for op die without a response.
func (s *Server) DropConnectionNext(op string, n int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.fails[op] = append(s.fails[op], failRule{times: n, closeConn: true})
}

// ClearFailures removes all injected failures.
func (s *Server) ClearFailures() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.fails = map[string][]failRule{}
}

// DropEventStreams closes every open SSE connection (clients should reconnect).
func (s *Server) DropEventStreams() {
	s.mu.Lock()
	old := s.kill
	s.kill = make(chan struct{})
	s.mu.Unlock()
	close(old)
}

// OpenEventStreams is the number of SSE connections currently open.
func (s *Server) OpenEventStreams() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.streams
}

// SSEConnects returns how clients connected to the event stream so far.
func (s *Server) SSEConnects() []SSEConnect {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]SSEConnect(nil), s.sseLog...)
}

// Requests returns a copy of the request log.
func (s *Server) Requests() []RequestRecord {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]RequestRecord(nil), s.reqLog...)
}

// CountRequests counts logged requests for an operation id.
func (s *Server) CountRequests(op string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	n := 0
	for _, r := range s.reqLog {
		if r.Op == op {
			n++
		}
	}
	return n
}

// Commit adds a changeset with the given full file snapshot and returns its id.
func (s *Server) Commit(repo string, parents []string, files map[string]string, author, message string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.commitLocked(s.repos[repo], parents, files, author, message)
}

func (s *Server) commitLocked(r *repoState, parents []string, files map[string]string, author, message string) string {
	h := sha256.New()
	fmt.Fprintf(h, "%v|%s|%s|%d\n", parents, author, message, len(r.changesets))
	keys := make([]string, 0, len(files))
	for k := range files {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	snap := map[string][]byte{}
	for _, k := range keys {
		fmt.Fprintf(h, "%s=%s\n", k, files[k])
		snap[k] = []byte(files[k])
	}
	id := hex.EncodeToString(h.Sum(nil))
	r.changesets[id] = &csState{id: id, parents: parents, files: snap, author: author, message: message, ts: s.nowMs()}
	return id
}

// SetHead moves a stream head (and bumps its version) as if a proposal landed.
func (s *Server) SetHead(repo, stream, head string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	st := r.streams[stream]
	if st == nil {
		st = &streamState{}
		r.streams[stream] = st
	}
	st.head = head
	st.version++
	s.emitLocked(r, "stream.integrate", "user:root", stream, map[string]any{"new_head": head, "version": st.version})
}

// AddTask adds a task directly (as if created by a human) and returns its id.
func (s *Server) AddTask(repo, title string, scope []string, deps ...string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	t := &taskState{Task: agwp.Task{ID: "task_" + randHex(6), Title: title, State: agwp.TaskOpen, Scope: scope, DependsOn: deps,
		CreatedBy: "user:root", CreatedAtMs: s.nowMs(), UpdatedAtMs: s.nowMs()}}
	r.tasks = append(r.tasks, t)
	s.emitLocked(r, "task.create", "user:root", t.ID, map[string]any{"title": title})
	return t.ID
}

// SetTaskDescription sets a task's description (fixtures for the scripted executor).
func (s *Server) SetTaskDescription(repo, id, desc string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if t := s.taskLocked(s.repos[repo], id); t != nil {
		t.Description = desc
	}
}

// Task returns a copy of a task.
func (s *Server) Task(repo, id string) agwp.Task {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.taskLocked(s.repos[repo], id)
	if t == nil {
		return agwp.Task{}
	}
	return s.viewTaskLocked(s.repos[repo], t)
}

// Runs returns copies of all runs of a repository.
func (s *Server) Runs(repo string) []agwp.Run {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []agwp.Run
	for _, r := range s.repos[repo].runs {
		out = append(out, *r)
	}
	return out
}

// Leases returns copies of all leases of a repository.
func (s *Server) Leases(repo string) []agwp.Lease {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []agwp.Lease
	for _, l := range s.repos[repo].leases {
		out = append(out, *l)
	}
	return out
}

// AddLease inserts a lease held by someone else (a fixture).
func (s *Server) AddLease(repo, holder, scope string, exclusive bool, ttl time.Duration) agwp.Lease {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	r.fence++
	l := &agwp.Lease{ID: "lease_" + randHex(6), Scope: scope, Holder: holder, Exclusive: exclusive, FencingToken: r.fence,
		AcquiredAtMs: s.nowMs(), ExpiresAtMs: s.now().Add(ttl).UnixMilli()}
	r.leases = append(r.leases, l)
	return *l
}

// Proposals returns copies of all proposals.
func (s *Server) Proposals(repo string) []agwp.Proposal {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []agwp.Proposal
	for _, p := range s.repos[repo].proposals {
		out = append(out, *p)
	}
	return out
}

// Reviews returns the reviews of a proposal.
func (s *Server) Reviews(repo, proposal string) []agwp.Review {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]agwp.Review(nil), s.repos[repo].reviews[proposal]...)
}

// AddProposal opens a proposal as a fixture.
func (s *Server) AddProposal(repo, author, stream, head, title, taskID string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.addProposalLocked(s.repos[repo], author, stream, head, title, taskID).ID
}

func (s *Server) addProposalLocked(r *repoState, author, stream, head, title, taskID string) *agwp.Proposal {
	p := &agwp.Proposal{ID: "prop_" + randHex(8), Stream: stream, Head: head, Title: title, Author: author,
		State: agwp.ProposalOpen, TaskID: taskID, CreatedAtMs: s.nowMs(), UpdatedAtMs: s.nowMs()}
	r.proposals = append(r.proposals, p)
	s.emitLocked(r, "proposal.create", author, p.ID, map[string]any{"head": head, "stream": stream})
	return p
}

// MoveProposal points a proposal at a new head (an update by its author).
func (s *Server) MoveProposal(repo, id, head string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	for _, p := range r.proposals {
		if p.ID == id {
			p.Head, p.UpdatedAtMs = head, s.nowMs()
			s.emitLocked(r, "proposal.update", p.Author, id, map[string]any{"head": head})
		}
	}
}

// IntegrateProposal marks a proposal integrated and moves the stream head to
// its head, as the real server would after a successful integrate.
func (s *Server) IntegrateProposal(repo, id string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	for _, p := range r.proposals {
		if p.ID == id {
			p.State, p.IntegratedAs = agwp.ProposalIntegrated, p.Head
			st := r.streams[p.Stream]
			st.head = p.Head
			st.version++
			s.emitLocked(r, "stream.integrate", "user:root", p.Stream, map[string]any{"new_head": p.Head, "proposal": id, "version": st.version})
		}
	}
}

// ClosePropsal closes a proposal without integrating it.
func (s *Server) CloseProposalFixture(repo, id string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	for _, p := range r.proposals {
		if p.ID == id {
			p.State = agwp.ProposalClosed
			s.emitLocked(r, "proposal.close", "user:root", id, map[string]any{})
		}
	}
}

// SetPreparation scripts the answer of prepare for a proposal.
func (s *Server) SetPreparation(repo, proposal string, p agwp.Preparation) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.repos[repo].prepares[proposal] = &p
}

// SetPolicy sets the tiny policy the default preparation evaluates.
func (s *Server) SetPolicy(repo, stream string, p Policy) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.repos[repo].policy[stream] = p
}

// Approve records an approval of the proposal's current head.
func (s *Server) Approve(repo, proposal, who string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	for _, p := range r.proposals {
		if p.ID == proposal {
			k := proposal + "@" + p.Head
			if r.approvals[k] == nil {
				r.approvals[k] = map[string]bool{}
			}
			r.approvals[k][who] = true
		}
	}
}

// Attest records a passing check for the candidate tree of a proposal.
func (s *Server) Attest(repo, tree, check string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	r := s.repos[repo]
	if r.attested[tree] == nil {
		r.attested[tree] = map[string]bool{}
	}
	r.attested[tree][check] = true
}

// SetPredict scripts the answer of predict.
func (s *Server) SetPredict(repo string, o []agwp.Overlap) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.repos[repo].predict = o
}

// Provenance returns the stored provenance records.
func (s *Server) Provenance(repo string) []agwp.ProvenanceEntry {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]agwp.ProvenanceEntry(nil), s.repos[repo].provenance...)
}

// Agents returns the registered agents.
func (s *Server) Agents(repo string) []agwp.Agent {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []agwp.Agent
	for _, a := range s.repos[repo].agents {
		out = append(out, *a)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out
}

// Emit appends a custom event to a repository's log.
func (s *Server) Emit(repo, topic, actor, target string, details map[string]any) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.emitLocked(s.repos[repo], topic, actor, target, details)
}

// LatestSeq is the newest event sequence number of a repository.
func (s *Server) LatestSeq(repo string) uint64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	return uint64(len(s.repos[repo].events))
}

// WorkspaceHead returns the head of a virtual workspace.
func (s *Server) WorkspaceHead(repo, name string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if w := s.repos[repo].workspaces[name]; w != nil {
		return w.Head
	}
	return ""
}

// File returns the content of a path at a changeset id.
func (s *Server) File(repo, rev, path string) ([]byte, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	cs := s.resolveLocked(s.repos[repo], rev)
	if cs == nil {
		return nil, false
	}
	b, ok := cs.files[path]
	return b, ok
}

func (s *Server) emitLocked(r *repoState, topic, actor, target string, details map[string]any) {
	if details == nil {
		details = map[string]any{}
	}
	d, _ := json.Marshal(details)
	r.events = append(r.events, agwp.Event{
		Seq: uint64(len(r.events) + 1), Topic: topic, Repo: r.name, TimestampMs: s.nowMs(),
		Payload: agwp.EventPayload{Actor: actor, Target: target, Details: d},
	})
	close(r.notify)
	r.notify = make(chan struct{})
}

func (s *Server) taskLocked(r *repoState, id string) *taskState {
	for _, t := range r.tasks {
		if t.ID == id {
			return t
		}
	}
	return nil
}

func (s *Server) viewTaskLocked(r *repoState, t *taskState) agwp.Task {
	v := t.Task
	v.Ready = true
	for _, d := range t.DependsOn {
		if dt := s.taskLocked(r, d); dt != nil && dt.State != agwp.TaskDone {
			v.Ready = false
		}
	}
	if v.DependsOn == nil {
		v.DependsOn = []string{}
	}
	if v.Scope == nil {
		v.Scope = []string{}
	}
	return v
}

func (s *Server) resolveLocked(r *repoState, rev string) *csState {
	if cs, ok := r.changesets[rev]; ok {
		return cs
	}
	if st, ok := r.streams[rev]; ok && st.head != "" {
		return r.changesets[st.head]
	}
	return nil
}

func randHex(n int) string {
	b := make([]byte, n)
	_, _ = rand.Read(b)
	return hex.EncodeToString(b)
}

func sortedKeys(m map[string]bool) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

// ---- HTTP plumbing --------------------------------------------------------------------

type apiErr struct {
	status       int
	code         string
	message      string
	retryable    bool
	retryAfterMs int64
}

func (e *apiErr) Error() string { return e.code + ": " + e.message }

func errf(status int, code, format string, a ...any) *apiErr {
	return &apiErr{status: status, code: code, message: fmt.Sprintf(format, a...)}
}

var (
	errNotFound = func(what string) *apiErr { return errf(404, "not_found", "%s not found", what) }
	errPrecond  = func(format string, a ...any) *apiErr { return errf(412, "failed_precondition", format, a...) }
	errInvalid  = func(format string, a ...any) *apiErr { return errf(400, "invalid_argument", format, a...) }
	errDenied   = func(format string, a ...any) *apiErr { return errf(403, "permission_denied", format, a...) }
)

type reqCtx struct {
	r        *http.Request
	body     []byte
	tok      *tokenInfo
	identity string // effective identity (with #run when acting as a run)
	run      string
	repo     *repoState
	q        url.Values
	rest     []string
	op       string
}

func (c *reqCtx) decode(v any) *apiErr {
	if len(strings.TrimSpace(string(c.body))) == 0 {
		return nil
	}
	dec := json.NewDecoder(strings.NewReader(string(c.body)))
	if err := dec.Decode(v); err != nil {
		return errInvalid("invalid JSON body: %v", err)
	}
	return nil
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

func (s *Server) writeErr(w http.ResponseWriter, reqID string, e *apiErr) {
	w.Header().Set("X-Request-Id", reqID)
	if e.retryAfterMs > 0 {
		w.Header().Set("Retry-After", strconv.FormatInt((e.retryAfterMs+999)/1000, 10))
	}
	retryable := e.retryable
	switch e.code {
	case "rate_limited", "unavailable", "stream_moved", "aborted":
		retryable = true
	}
	writeJSON(w, e.status, map[string]any{"error": map[string]any{
		"code": e.code, "message": e.message, "retryable": retryable, "retry_after_ms": e.retryAfterMs,
		"request_id": reqID, "details": map[string]string{},
	}})
}

func (s *Server) serve(w http.ResponseWriter, r *http.Request) {
	body, _ := io.ReadAll(io.LimitReader(r.Body, 16<<20))
	reqID := r.Header.Get("X-Request-Id")
	if reqID == "" {
		reqID = "req_" + randHex(6)
	}
	op, repoName, rest := route(r.Method, r.URL.Path)

	s.mu.Lock()
	rec := RequestRecord{Method: r.Method, Path: r.URL.Path, Query: r.URL.RawQuery, Header: r.Header.Clone(), Body: string(body), Op: op, Run: r.Header.Get("Ag-Run")}
	// injected failures
	var rule *failRule
	for _, key := range []string{op, "*"} {
		rules := s.fails[key]
		for i := range rules {
			if rules[i].times != 0 {
				rule = &rules[i]
				if rules[i].times > 0 {
					rules[i].times--
				}
				break
			}
		}
		if rule != nil {
			break
		}
	}
	tok := s.authLocked(r)
	if tok != nil {
		rec.Identity = tok.identity
	}
	s.reqLog = append(s.reqLog, rec)
	s.mu.Unlock()

	if rule != nil {
		if rule.closeConn {
			if hj, ok := w.(http.Hijacker); ok {
				if conn, _, err := hj.Hijack(); err == nil {
					conn.Close()
					return
				}
			}
		}
		s.writeErr(w, reqID, &apiErr{status: rule.status, code: rule.code, message: rule.message, retryable: rule.retryable, retryAfterMs: rule.retryAfterMs})
		return
	}
	if op == "" {
		s.writeErr(w, reqID, errNotFound("route "+r.URL.Path))
		return
	}
	if op == "session.negotiate" {
		writeJSON(w, 200, agwp.Capabilities{ProtocolVersion: agwp.ProtocolVersion, ServerVersion: "fake", Features: []string{"events.replay", "workspaces.virtual", "auth.scoped_credentials"}, AuthMethods: []string{"bearer"}})
		return
	}
	if tok == nil {
		s.writeErr(w, reqID, errf(401, "unauthenticated", "missing, invalid, expired or revoked credential"))
		return
	}
	c := &reqCtx{r: r, body: body, tok: tok, identity: tok.identity, q: r.URL.Query(), rest: rest, op: op}
	if op == "session.whoami" {
		s.mu.Lock()
		p := agwp.Principal{Identity: tok.identity, Kind: kindOf(tok.identity), Method: "token", CredentialID: tok.id, Roles: []string{"*:" + tok.role},
			Scope: agwp.CredentialScope{Repos: nonNil(tok.repos), Streams: nonNil(tok.streams), Operations: sortedKeys(tok.ops)}}
		if !tok.expiresAt.IsZero() {
			p.ExpiresAtMs = tok.expiresAt.UnixMilli()
		}
		s.mu.Unlock()
		writeJSON(w, 200, p)
		return
	}
	cap := capOf(op)
	s.mu.Lock()
	if repoName != "" {
		c.repo = s.repos[repoName]
		if c.repo == nil {
			s.mu.Unlock()
			s.writeErr(w, reqID, errNotFound("repository '"+repoName+"'"))
			return
		}
		if !roleAllows(tok.role, cap) {
			s.mu.Unlock()
			s.writeErr(w, reqID, errDenied("this identity (%s) may not perform '%s' on '%s' (roles: %s)", tok.identity, cap, repoName, tok.role))
			return
		}
		if !tok.permits(repoName, cap) {
			s.mu.Unlock()
			s.writeErr(w, reqID, errDenied("the credential presented is not scoped for '%s' on '%s'", cap, repoName))
			return
		}
	} else if !roleAllows(tok.role, cap) || !tok.permits("*", cap) {
		s.mu.Unlock()
		s.writeErr(w, reqID, errDenied("'%s' requires a global role", cap))
		return
	}
	// acting as an agent run
	if run := r.Header.Get("Ag-Run"); run != "" && c.repo != nil {
		if kindOf(tok.identity) != "agent" {
			s.mu.Unlock()
			s.writeErr(w, reqID, errInvalid("only agent identities may act as a run"))
			return
		}
		var found *agwp.Run
		for _, ru := range c.repo.runs {
			if ru.RunID == run {
				found = ru
			}
		}
		switch {
		case found == nil:
			s.mu.Unlock()
			s.writeErr(w, reqID, errNotFound("run '"+run+"'"))
			return
		case "agent:"+found.Agent != tok.identity:
			s.mu.Unlock()
			s.writeErr(w, reqID, errDenied("this run belongs to a different agent"))
			return
		case found.Status != "running":
			s.mu.Unlock()
			s.writeErr(w, reqID, errPrecond("run '%s' is %s", run, found.Status))
			return
		}
		c.identity, c.run = tok.identity+"#"+run, run
	}
	if op == "events.subscribe" {
		s.mu.Unlock()
		s.sse(w, r, c)
		return
	}
	resp, status, aerr := s.handle(c)
	s.mu.Unlock()
	if aerr != nil {
		s.writeErr(w, reqID, aerr)
		return
	}
	w.Header().Set("X-Request-Id", reqID)
	if raw, ok := resp.([]byte); ok {
		w.Header().Set("Content-Type", "application/octet-stream")
		w.WriteHeader(200)
		_, _ = w.Write(raw)
		return
	}
	if status == 0 {
		status = 200
	}
	writeJSON(w, status, resp)
}

func (s *Server) authLocked(r *http.Request) *tokenInfo {
	h := r.Header.Get("Authorization")
	scheme, tok, ok := strings.Cut(h, " ")
	if !ok || !strings.EqualFold(scheme, "bearer") {
		return nil
	}
	t := s.tokens[strings.TrimSpace(tok)]
	if t == nil || t.revoked || (!t.expiresAt.IsZero() && !s.now().Before(t.expiresAt)) {
		return nil
	}
	return t
}

func (t *tokenInfo) permits(repo, cap string) bool {
	if len(t.ops) > 0 && !t.ops[cap] {
		return false
	}
	if len(t.repos) > 0 && repo != "*" {
		ok := false
		for _, p := range t.repos {
			if matchSimple(p, repo) {
				ok = true
			}
		}
		if !ok {
			return false
		}
	}
	return true
}

func matchSimple(pattern, s string) bool {
	if pattern == "*" || pattern == s {
		return true
	}
	if strings.HasSuffix(pattern, "*") {
		return strings.HasPrefix(s, strings.TrimSuffix(pattern, "*"))
	}
	return false
}

func kindOf(identity string) string {
	k, _, _ := strings.Cut(identity, ":")
	return k
}

func principalOf(identity string) string {
	p, _, _ := strings.Cut(identity, "#")
	return p
}

func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}

// route maps a request to (operation id, repo, remaining segments).
func route(method, p string) (op, repo string, rest []string) {
	segs := strings.Split(strings.Trim(p, "/"), "/")
	if len(segs) < 2 || segs[0] != "v1" {
		return "", "", nil
	}
	switch {
	case p == "/v1/capabilities":
		return "session.negotiate", "", nil
	case p == "/v1/whoami":
		return "session.whoami", "", nil
	case p == "/v1/credentials" && method == "POST":
		return "admin.credentials.mint", "", nil
	case len(segs) == 3 && segs[1] == "credentials" && method == "DELETE":
		return "admin.credentials.revoke", "", segs[2:]
	case p == "/v1/repos" && method == "POST":
		return "repo.create", "", nil
	case p == "/v1/roles":
		return "admin.roles.grant", "", nil
	}
	if segs[1] != "repos" || len(segs) < 4 {
		return "", "", nil
	}
	repo = segs[2] + "/" + segs[3]
	rest = segs[4:]
	key := strings.Join(rest, "/")
	m := method
	switch {
	case key == "" && m == "GET":
		op = "repo.describe"
	case key == "streams" && m == "GET":
		op = "stream.list"
	case key == "streams/head":
		op = "stream.head"
	case key == "agents" && m == "GET":
		op = "agent.list"
	case key == "agents" && m == "POST":
		op = "agent.register"
	case key == "runs" && m == "GET":
		op = "agent.list_runs"
	case key == "runs" && m == "POST":
		op = "agent.start_run"
	case len(rest) == 3 && rest[0] == "runs" && rest[2] == "heartbeat":
		op = "agent.heartbeat"
	case len(rest) == 3 && rest[0] == "runs" && rest[2] == "complete":
		op = "agent.complete"
	case key == "tasks" && m == "GET":
		op = "task.list"
	case key == "tasks" && m == "POST":
		op = "task.create"
	case len(rest) == 2 && rest[0] == "tasks" && m == "GET":
		op = "task.get"
	case len(rest) == 3 && rest[0] == "tasks" && rest[2] == "finish":
		op = "task.finish"
	case len(rest) == 3 && rest[0] == "tasks" && rest[2] == "dependencies":
		op = "task.depend"
	case key == "leases" && m == "GET":
		op = "lease.list"
	case key == "leases" && m == "POST":
		op = "lease.acquire"
	case len(rest) == 2 && rest[0] == "leases" && m == "DELETE":
		op = "lease.release"
	case len(rest) == 3 && rest[0] == "leases" && rest[2] == "extend":
		op = "lease.extend"
	case key == "predict":
		op = "agent.predict"
	case key == "workspaces" && m == "POST":
		op = "workspace.create"
	case len(rest) == 2 && rest[0] == "workspaces" && m == "GET":
		op = "workspace.get"
	case len(rest) == 2 && rest[0] == "workspaces" && m == "DELETE":
		op = "workspace.delete"
	case len(rest) == 3 && rest[0] == "workspaces" && rest[2] == "changes":
		op = "workspace.modify"
	case len(rest) == 3 && rest[0] == "workspaces" && rest[2] == "sync":
		op = "workspace.sync"
	case key == "proposals" && m == "GET":
		op = "proposal.list"
	case key == "proposals" && m == "POST":
		op = "proposal.create"
	case len(rest) == 2 && rest[0] == "proposals" && m == "GET":
		op = "proposal.get"
	case len(rest) == 3 && rest[0] == "proposals" && rest[2] == "reviews":
		op = "proposal.review"
	case len(rest) == 3 && rest[0] == "proposals" && rest[2] == "close":
		op = "proposal.close"
	case len(rest) == 3 && rest[0] == "proposals" && rest[2] == "prepare":
		op = "integration.prepare"
	case len(rest) == 3 && rest[0] == "proposals" && rest[2] == "integrate":
		op = "integration.integrate"
	case key == "policy" && m == "PUT":
		op = "admin.policy.set"
	case key == "queue":
		op = "queue.list"
	case key == "tree" && m == "GET":
		op = "tree.get"
	case len(rest) >= 2 && rest[0] == "files":
		op = "tree.file"
	case key == "history/resolve":
		op = "history.resolve"
	case key == "provenance" && m == "POST":
		op = "provenance.submit"
	case key == "provenance" && m == "GET":
		op = "provenance.list"
	case key == "events/replay":
		op = "events.replay"
	case key == "events":
		op = "events.subscribe"
	case key == "jobs":
		op = "jobs.list"
	}
	return op, repo, rest
}

func capOf(op string) string {
	m := map[string]string{
		"repo.describe": "repo.read", "stream.list": "stream.read", "stream.head": "stream.read", "agent.list": "agent.read",
		"agent.register": "agent.register", "agent.list_runs": "agent.read", "agent.start_run": "agent.run",
		"agent.heartbeat": "agent.run", "agent.complete": "agent.run", "task.list": "task.read", "task.get": "task.read",
		"task.create": "task.write", "task.finish": "task.write", "task.depend": "task.write", "lease.list": "lease.read",
		"lease.acquire": "lease.write", "lease.release": "lease.write", "lease.extend": "lease.write", "agent.predict": "predict",
		"workspace.create": "workspace.write", "workspace.get": "workspace.read", "workspace.delete": "workspace.write",
		"workspace.modify": "workspace.write", "workspace.sync": "workspace.write", "proposal.list": "proposal.read",
		"proposal.create": "proposal.create", "proposal.get": "proposal.read", "proposal.review": "proposal.review",
		"proposal.close": "proposal.close", "integration.prepare": "integrate.prepare", "integration.integrate": "integrate.run",
		"admin.policy.set": "policy.write", "queue.list": "queue.read", "tree.get": "tree.read", "tree.file": "tree.read",
		"history.resolve": "history.read", "provenance.submit": "provenance.write", "provenance.list": "provenance.read",
		"events.replay": "events.read", "events.subscribe": "events.read", "jobs.list": "jobs.read",
		"admin.credentials.mint": "credentials.mint", "admin.credentials.revoke": "credentials.revoke",
		"repo.create": "repo.create", "admin.roles.grant": "roles.manage",
	}
	return m[op]
}

var readerCaps = []string{"repo.read", "objects.read", "operation.read", "history.read", "tree.read", "stream.read", "workspace.read",
	"proposal.read", "integrate.prepare", "queue.read", "attestation.read", "agent.read", "task.read", "lease.read", "provenance.read",
	"predict", "events.read", "policy.read", "pins.read", "jobs.read"}

var writerCaps = []string{"objects.write", "publish", "stream.create", "workspace.write", "proposal.create", "proposal.update",
	"proposal.close", "integrate.validate", "queue.write", "agent.register", "agent.run", "task.write", "lease.write", "provenance.write"}

func roleAllows(role, cap string) bool {
	has := func(list []string) bool {
		for _, c := range list {
			if c == cap {
				return true
			}
		}
		return false
	}
	switch role {
	case "admin":
		return true
	case "reader":
		return has(readerCaps)
	case "writer":
		return has(readerCaps) || has(writerCaps)
	case "reviewer":
		return has(readerCaps) || cap == "proposal.review"
	case "integrator":
		return has(readerCaps) || cap == "integrate.run" || cap == "queue.run" || cap == "proposal.close"
	}
	return false
}
