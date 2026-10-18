package session

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

// Options configures a Manager.
type Options struct {
	// Orch is the orchestrator's own AGWP client (service identity). It starts
	// runs, mints agent credentials and completes runs; it is never handed to an executor.
	Orch *agwp.Client
	// ServerURL is what executors are told as AG_SERVER.
	ServerURL string
	Keys      *agentkey.Store
	// WorkRoot holds one private directory per session.
	WorkRoot string
	Clock    clock.Clock
	Metrics  *metrics.Service
	Log      *slog.Logger
	// ParentEnv reads the orchestrator's environment for pass_env (default: none).
	ParentEnv func(string) string
	// OnDone is called after a session fully ended (the scheduler kicks itself).
	OnDone func(repo string)
}

// Manager starts, supervises and accounts for sessions.
type Manager struct {
	o    Options
	root context.Context
	stop context.CancelFunc

	mu       sync.Mutex
	closed   bool
	sessions map[string]*Session // by run id
	slots    map[string]int      // running sessions per executor
	agents   map[string]string   // repo|agent -> registered public key
	wg       sync.WaitGroup
}

// NewManager builds a Manager.
func NewManager(o Options) (*Manager, error) {
	if o.Orch == nil || o.Keys == nil || o.WorkRoot == "" {
		return nil, errors.New("session: Orch, Keys and WorkRoot are required")
	}
	if o.Clock == nil {
		o.Clock = clock.Real{}
	}
	if o.Log == nil {
		o.Log = slog.Default()
	}
	if o.Metrics == nil {
		o.Metrics = metrics.NewService(metrics.NewRegistry())
	}
	if o.ParentEnv == nil {
		o.ParentEnv = func(string) string { return "" }
	}
	if err := os.MkdirAll(o.WorkRoot, 0o700); err != nil {
		return nil, err
	}
	root, stop := context.WithCancel(context.Background())
	return &Manager{o: o, root: root, stop: stop, sessions: map[string]*Session{}, slots: map[string]int{}, agents: map[string]string{}}, nil
}

func newExecutor(c config.Executor) Executor {
	if c.Type == "scripted" {
		return scriptedExecutor{cfg: c.Script}
	}
	return commandExecutor{cfg: c}
}

// FreeSlots is how many more sessions the executor may start.
func (m *Manager) FreeSlots(c config.Executor) int {
	m.mu.Lock()
	defer m.mu.Unlock()
	return max(0, c.Slots-m.slots[c.Name])
}

// Active returns snapshots of the running sessions, oldest first.
func (m *Manager) Active() []Info {
	m.mu.Lock()
	ss := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		ss = append(ss, s)
	}
	m.mu.Unlock()
	out := make([]Info, 0, len(ss))
	for _, s := range ss {
		out = append(out, s.Info())
	}
	sort.Slice(out, func(i, j int) bool {
		if !out[i].StartedAt.Equal(out[j].StartedAt) {
			return out[i].StartedAt.Before(out[j].StartedAt)
		}
		return out[i].RunID < out[j].RunID
	})
	return out
}

// Has reports whether a run is supervised by this manager.
func (m *Manager) Has(runID string) bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	_, ok := m.sessions[runID]
	return ok
}

// Cancel asks a session's executor to stop. It reports whether the run is ours.
func (m *Manager) Cancel(runID, reason string) bool {
	m.mu.Lock()
	s := m.sessions[runID]
	m.mu.Unlock()
	if s == nil {
		return false
	}
	s.stop(OutcomeCancelled, reason)
	return true
}

// Start prepares a session synchronously (agent, run, credential, leases) and
// then supervises the executor in the background. On error nothing is left
// running; a run that was already started on the server is completed as failed.
func (m *Manager) Start(ctx context.Context, req Request) (*Info, error) {
	ex := req.Executor
	m.mu.Lock()
	if m.closed {
		m.mu.Unlock()
		return nil, ErrClosed
	}
	if m.slots[ex.Name] >= ex.Slots {
		m.mu.Unlock()
		return nil, ErrNoSlot
	}
	m.slots[ex.Name]++
	m.wg.Add(1)
	m.mu.Unlock()
	registered := false
	defer func() {
		if !registered {
			m.mu.Lock()
			m.slots[ex.Name]--
			m.mu.Unlock()
			m.wg.Done()
		}
	}()

	key, err := m.o.Keys.Ensure(ex.Agent)
	if err != nil {
		return nil, fmt.Errorf("agent key: %w", err)
	}
	if err := m.ensureAgent(ctx, req.Repo, ex.Agent, key.PublicHex()); err != nil {
		return nil, err
	}
	run, err := m.o.Orch.StartRun(ctx, req.Repo, ex.Agent, req.Task.ID)
	if err != nil {
		if agwp.IsCode(err, agwp.CodeFailedPrecond, agwp.CodeNotFound, agwp.CodeAlreadyExists, agwp.CodeAborted) {
			return nil, fmt.Errorf("%w: %v", ErrTaskUnavailable, err)
		}
		return nil, fmt.Errorf("start run: %w", err)
	}
	s := newSession(m, req, run, key)
	fail := func(cause error, summary string) (*Info, error) {
		m.completeRun(run.RunID, req.Repo, "failed", summary)
		s.cleanup()
		return nil, cause
	}

	// The credential is bound to this run: it acts only as the run, only in this repository, and stops working
	// when the run is no longer running - so concurrent sessions of one agent cannot act for each other.
	minted, err := m.o.Orch.MintCredential(ctx, agwp.MintRequest{
		Subject:     "agent:" + ex.Agent,
		Description: fmt.Sprintf("session %s task %s", run.RunID, req.Task.ID),
		TTLSecs:     uint32(ex.Cred / time.Second),
		Scope: agwp.CredentialScope{
			Repos: []string{req.Repo}, Streams: []string{req.Stream}, Operations: ex.Ops, RunID: run.RunID,
		},
	})
	if err != nil {
		return fail(fmt.Errorf("mint credential: %w", err), "could not mint the session credential: "+err.Error())
	}
	m.o.Metrics.CredentialsMinted.With(req.Repo).Inc()
	s.credID, s.token = minted.Credential.ID, minted.Token
	// A server that does not know about run binding would mint an ordinary agent credential and say nothing. An
	// unbound credential is never handed to an executor.
	if minted.Credential.Scope.RunID != run.RunID {
		s.revoke()
		return fail(errors.New("the server minted a credential that is not bound to the run (it predates run-bound credentials); upgrade ag-server"),
			"the server does not bind credentials to runs")
	}
	s.agent = m.o.Orch.WithToken(minted.Token, "agent:"+ex.Agent)
	s.runClient = s.agent.WithRun(run.RunID)

	if err := s.takeLeases(ctx); err != nil {
		s.revoke()
		if errors.Is(err, ErrLeaseConflict) {
			return fail(err, "deferred: "+err.Error())
		}
		return fail(err, "could not take the task's leases: "+err.Error())
	}
	if err := s.prepareWorkDir(); err != nil {
		s.revoke()
		return fail(err, "could not prepare the work directory: "+err.Error())
	}

	m.mu.Lock()
	m.sessions[run.RunID] = s
	registered = true
	closing := m.closed // a shutdown began while this session was being prepared
	m.mu.Unlock()
	m.o.Metrics.SessionsStarted.With(req.Repo, ex.Name).Inc()
	m.o.Metrics.SessionsRunning.With(req.Repo, ex.Name).Inc()
	m.o.Log.Info("session started", "repo", req.Repo, "executor", ex.Name, "agent", ex.Agent, "run", run.RunID, "task", req.Task.ID, "attempt", req.Attempt)
	go s.supervise(m.root)
	if closing {
		s.stop(OutcomeCancelled, "orchestrator shutdown")
	}
	info := s.Info()
	return &info, nil
}

// ensureAgent registers the agent with its public key unless it already is.
func (m *Manager) ensureAgent(ctx context.Context, repo, agent, pub string) error {
	k := repo + "|" + agent
	m.mu.Lock()
	known := m.agents[k]
	m.mu.Unlock()
	if known == pub {
		return nil
	}
	agents, err := m.o.Orch.ListAgents(ctx, repo)
	if err != nil {
		return fmt.Errorf("list agents: %w", err)
	}
	for _, a := range agents {
		if a.Name == agent {
			return m.noteAgent(k, a, pub)
		}
	}
	a, err := m.o.Orch.RegisterAgent(ctx, repo, agent, pub)
	if err != nil {
		if agwp.IsCode(err, agwp.CodeAlreadyExists) { // raced with someone else; re-read
			again, lerr := m.o.Orch.ListAgents(ctx, repo)
			if lerr == nil {
				for _, x := range again {
					if x.Name == agent {
						return m.noteAgent(k, x, pub)
					}
				}
			}
		}
		return fmt.Errorf("register agent %q: %w", agent, err)
	}
	return m.noteAgent(k, *a, pub)
}

func (m *Manager) noteAgent(k string, a agwp.Agent, pub string) error {
	if a.Disabled {
		return fmt.Errorf("agent %q is disabled on the server", a.Name)
	}
	if a.PublicKey != pub {
		return fmt.Errorf("agent %q is registered with a different public key than the one in the key store (%s); refusing to start: provenance would not verify. Re-register the agent or restore its key", a.Name, a.PublicKey[:min(12, len(a.PublicKey))])
	}
	m.mu.Lock()
	m.agents[k] = pub
	m.mu.Unlock()
	return nil
}

// completeRun ends a run on the server with a few retries; failures are logged.
func (m *Manager) completeRun(runID, repo, status, summary string) {
	if len(summary) > 2000 {
		summary = summary[:2000] + "..."
	}
	var err error
	for attempt := 0; attempt < 3; attempt++ {
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		_, err = m.o.Orch.CompleteRun(ctx, repo, runID, status, summary)
		cancel()
		if err == nil || agwp.IsNotFound(err) {
			return
		}
		_ = clock.Sleep(context.Background(), m.o.Clock, time.Duration(attempt+1)*200*time.Millisecond)
	}
	m.o.Log.Error("could not complete the run; it stays running until it is reaped", "repo", repo, "run", runID, "err", err)
}

// FinishTask marks a task done or cancelled as an operator. The server lets the orchestrator (the delegator
// role: capability agent.operate) finish a task that an agent owns, and records in its audit log that an
// operator did, and whose task it was. No credential is minted for it.
func (m *Manager) FinishTask(ctx context.Context, repo, taskID, state string) error {
	_, err := m.o.Orch.FinishTask(ctx, repo, taskID, state)
	return err
}

// Shutdown stops accepting sessions and ends the running ones. With wait, it
// lets them finish until ctx expires and cancels the rest; with cancel it
// stops them at once. It returns when every session has been cleaned up.
func (m *Manager) Shutdown(ctx context.Context, wait bool) {
	m.mu.Lock()
	m.closed = true
	live := make([]*Session, 0, len(m.sessions))
	for _, s := range m.sessions {
		live = append(live, s)
	}
	m.mu.Unlock()
	done := make(chan struct{})
	go func() { m.wg.Wait(); close(done) }()
	if wait {
		select {
		case <-done:
			m.stop()
			return
		case <-ctx.Done():
		}
	}
	for _, s := range live {
		s.stop(OutcomeCancelled, "orchestrator shutdown")
	}
	<-done // sessions bound their own cleanup (grace + a few API calls)
	m.stop()
}

// Close is Shutdown(cancel) with a short deadline, for tests.
func (m *Manager) Close() {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	m.Shutdown(ctx, false)
}

// writeTaskFile stores the task as JSON for the executor (title and
// description are untrusted text).
func writeTaskFile(path string, t agwp.Task) error {
	b, err := json.MarshalIndent(t, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, b, 0o600)
}

func (m *Manager) workDir(runID string) string { return filepath.Join(m.o.WorkRoot, runID) }
