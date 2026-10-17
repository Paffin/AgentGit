package session

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// Session is one agent run being supervised.
type Session struct {
	m    *Manager
	req  Request
	run  *agwp.Run
	key  *agentkey.Key
	exec Executor

	started time.Time
	done    chan struct{}

	credID    string
	token     secret.Secret
	agent     *agwp.Client // agent credential, no run header
	runClient *agwp.Client // agent credential acting as the run

	workDir, taskFile, resultFile, keyFile string
	stdout, stderr                         *tail

	execCtx    context.Context
	cancelExec context.CancelFunc

	mu          sync.Mutex
	leases      []LeaseInfo
	leaseIDs    []string
	stopOutcome Outcome
	stopReason  string
	lastBeat    time.Time
	beatFails   int
}

func newSession(m *Manager, req Request, run *agwp.Run, key *agentkey.Key) *Session {
	ctx, cancel := context.WithCancel(context.Background())
	out := req.Executor.Output
	return &Session{
		m: m, req: req, run: run, key: key, exec: newExecutor(req.Executor), started: m.o.Clock.Now(),
		done: make(chan struct{}), execCtx: ctx, cancelExec: cancel,
		stdout: newTail(out), stderr: newTail(out), lastBeat: m.o.Clock.Now(),
	}
}

// Done is closed when the session has fully ended.
func (s *Session) Done() <-chan struct{} { return s.done }

// Info returns a snapshot without secrets.
func (s *Session) Info() Info {
	s.mu.Lock()
	defer s.mu.Unlock()
	state := "running"
	if s.stopOutcome != "" {
		state = "stopping"
	}
	return Info{
		Repo: s.req.Repo, Executor: s.req.Executor.Name, Agent: s.req.Executor.Agent, RunID: s.run.RunID,
		TaskID: s.req.Task.ID, TaskTitle: s.req.Task.Title, Scope: s.req.Task.Scope, Attempt: s.req.Attempt,
		State: state, StartedAt: s.started, LastHeartbeat: s.lastBeat, Leases: append([]LeaseInfo(nil), s.leases...),
		StopReason: s.stopReason, StderrTail: secret.Scrub(s.stderr.Last(512), s.token),
	}
}

// stop asks the executor to stop; the first request wins.
func (s *Session) stop(outcome Outcome, reason string) {
	s.mu.Lock()
	if s.stopOutcome == "" {
		s.stopOutcome, s.stopReason = outcome, reason
	}
	s.mu.Unlock()
	s.cancelExec()
}

func (s *Session) stopRequest() (Outcome, string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.stopOutcome, s.stopReason
}

func (s *Session) takeLeases(ctx context.Context) error {
	if s.req.Lease == "none" || s.req.Lease == "" || len(s.req.Task.Scope) == 0 {
		return nil
	}
	for _, glob := range s.req.Task.Scope {
		got, err := s.runClient.AcquireLease(ctx, s.req.Repo, glob, s.req.Lease == "exclusive", s.req.LeaseTTL, s.req.Task.ID)
		if err != nil {
			if agwp.IsCode(err, agwp.CodeLeaseConflict) {
				return fmt.Errorf("%w: %v", ErrLeaseConflict, err)
			}
			return err
		}
		s.mu.Lock()
		s.leases = append(s.leases, LeaseInfo{ID: got.Lease.ID, Scope: got.Lease.Scope, Exclusive: got.Lease.Exclusive, Token: got.Lease.FencingToken})
		s.leaseIDs = append(s.leaseIDs, got.Lease.ID)
		s.mu.Unlock()
	}
	return nil
}

func (s *Session) prepareWorkDir() error {
	s.workDir = s.m.workDir(s.run.RunID)
	if err := os.MkdirAll(s.workDir, 0o700); err != nil {
		return err
	}
	s.taskFile = filepath.Join(s.workDir, "task.json")
	s.resultFile = filepath.Join(s.workDir, "result.json")
	s.keyFile = s.m.o.Keys.Path(s.req.Executor.Agent)
	return writeTaskFile(s.taskFile, s.req.Task)
}

// cleanup removes local traces of a session that never got going.
func (s *Session) cleanup() {
	if s.workDir != "" {
		_ = os.RemoveAll(s.workDir)
	}
}

func (s *Session) env() Env {
	return Env{
		Clock: s.m.o.Clock, Log: s.m.o.Log.With("run", s.run.RunID),
		Repo: s.req.Repo, Stream: s.req.Stream, Task: s.req.Task, Agent: s.req.Executor.Agent,
		RunID: s.run.RunID, Identity: s.run.Identity, Server: s.m.o.ServerURL, Token: s.token,
		KeyFile: s.keyFile, WorkDir: s.workDir, TaskFile: s.taskFile, ResultFile: s.resultFile,
		Client: s.runClient, Stdout: s.stdout, Stderr: s.stderr, Grace: s.req.Executor.Grace, ParentEnv: s.m.o.ParentEnv,
	}
}

// supervise runs the executor, keeps the run alive and ends the session.
func (s *Session) supervise(root context.Context) {
	defer close(s.done)
	clk := s.m.o.Clock
	ex := s.req.Executor
	exitCh := make(chan Exit, 1)
	go func() {
		defer func() {
			if p := recover(); p != nil {
				exitCh <- Exit{Code: -1, Err: fmt.Errorf("executor panicked: %v", p)}
			}
		}()
		exitCh <- s.exec.Run(s.execCtx, s.env())
	}()
	go func() { // the manager's shutdown also stops the executor
		select {
		case <-root.Done():
			s.stop(OutcomeCancelled, "orchestrator stopped")
		case <-s.done:
		}
	}()

	beat := clk.NewTicker(ex.Beat)
	defer beat.Stop()
	timeout := clk.NewTimer(ex.Timeout)
	defer timeout.Stop()
	var exit Exit
loop:
	for {
		select {
		case exit = <-exitCh:
			break loop
		case <-beat.C():
			s.heartbeat()
		case <-timeout.C():
			s.stop(OutcomeTimeout, fmt.Sprintf("time limit of %s exceeded", ex.Timeout))
		}
	}
	s.finish(exit)
}

func (s *Session) heartbeat() {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	err := s.m.o.Orch.Heartbeat(ctx, s.req.Repo, s.run.RunID)
	if err != nil {
		s.mu.Lock()
		s.beatFails++
		n := s.beatFails
		s.mu.Unlock()
		s.m.o.Log.Warn("heartbeat failed", "run", s.run.RunID, "failures", n, "err", err)
		if agwp.IsCode(err, agwp.CodeNotFound, agwp.CodeFailedPrecond) {
			// The server ended the run (reaped, or completed by someone else): the credential is
			// about to stop working too, so there is nothing left to supervise.
			s.stop(OutcomeCancelled, "the server no longer considers the run running: "+err.Error())
		}
		return
	}
	s.mu.Lock()
	s.beatFails = 0
	s.lastBeat = s.m.o.Clock.Now()
	ids := append([]string(nil), s.leaseIDs...)
	s.mu.Unlock()
	for _, id := range ids {
		if _, err := s.runClient.ExtendLease(ctx, s.req.Repo, id, s.req.LeaseTTL); err != nil {
			s.m.o.Log.Warn("could not extend a lease", "run", s.run.RunID, "lease", id, "err", err)
		}
	}
}

// finish turns the executor's exit into the run's final state on the server,
// applies the task's disposition and tears the session down.
func (s *Session) finish(exit Exit) {
	m := s.m
	defer func() {
		s.cleanup()
		m.mu.Lock()
		delete(m.sessions, s.run.RunID)
		m.slots[s.req.Executor.Name]--
		m.mu.Unlock()
		m.o.Metrics.SessionsRunning.With(s.req.Repo, s.req.Executor.Name).Dec()
		m.wg.Done()
		if m.o.OnDone != nil {
			m.o.OnDone(s.req.Repo)
		}
	}()

	res := s.result(exit)
	status := "failed"
	if res.Outcome == OutcomeCompleted {
		status = "completed"
	}
	disposition := Keep
	if s.req.Decide != nil {
		disposition = s.req.Decide(res)
	}
	m.completeRun(s.run.RunID, s.req.Repo, status, res.Summary)
	if disposition != Keep {
		state := "done"
		if disposition == Cancel {
			state = "cancelled"
		}
		s.finishTask(state)
	}
	s.revoke()
	m.o.Metrics.SessionsFinished.With(s.req.Repo, s.req.Executor.Name, string(res.Outcome)).Inc()
	m.o.Log.Info("session ended", "repo", s.req.Repo, "executor", s.req.Executor.Name, "run", s.run.RunID,
		"task", s.req.Task.ID, "outcome", res.Outcome, "exit", res.ExitCode, "signal", res.Signal, "task_disposition", disposition.String(),
		"duration", res.Duration.Round(time.Millisecond))
}

func (s *Session) finishTask(state string) {
	var err error
	for attempt := 0; attempt < 3; attempt++ {
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		_, err = s.agent.FinishTask(ctx, s.req.Repo, s.req.Task.ID, state)
		cancel()
		if err == nil {
			return
		}
		if agwp.IsCode(err, agwp.CodeFailedPrecond) { // finished or owned by someone else in the meantime
			s.m.o.Log.Info("task already finished or no longer ours", "task", s.req.Task.ID, "err", err)
			return
		}
		time.Sleep(time.Duration(attempt+1) * 200 * time.Millisecond)
	}
	s.m.o.Log.Error("could not mark the task; it stays in progress", "task", s.req.Task.ID, "state", state, "err", err)
}

func (s *Session) revoke() {
	if s.credID == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := s.m.o.Orch.RevokeCredential(ctx, s.credID); err != nil {
		s.m.o.Log.Warn("could not revoke the session credential; it expires on its own", "credential", s.credID, "err", err)
	}
}

// result classifies the exit and builds the run summary.
func (s *Session) result(exit Exit) Result {
	ex := s.req.Executor
	stopOutcome, stopReason := s.stopRequest()
	res := Result{
		Repo: s.req.Repo, Executor: ex.Name, Agent: ex.Agent, RunID: s.run.RunID, Task: s.req.Task, Attempt: s.req.Attempt,
		ExitCode: exit.Code, Signal: exit.Signal, Duration: s.m.o.Clock.Now().Sub(s.started), Reason: stopReason,
	}
	switch {
	case stopOutcome != "":
		res.Outcome = stopOutcome
	case exit.OK():
		res.Outcome = OutcomeCompleted
	default:
		res.Outcome = OutcomeFailed
	}
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	if props, err := s.m.o.Orch.ListProposals(ctx, s.req.Repo, agwp.ProposalFilter{Author: s.run.Identity}); err == nil {
		res.Proposals = props
	} else {
		s.m.o.Log.Warn("could not list the proposals of a finished run", "run", s.run.RunID, "err", err)
	}

	var b strings.Builder
	switch res.Outcome {
	case OutcomeCompleted:
		fmt.Fprintf(&b, "completed by executor %s (attempt %d, %s)", ex.Name, s.req.Attempt, res.Duration.Round(time.Millisecond))
		if sum := s.readResult(); sum != "" {
			b.WriteString(": " + sum)
		}
	case OutcomeTimeout:
		fmt.Fprintf(&b, "timeout: %s; the executor was stopped (attempt %d)", stopReason, s.req.Attempt)
	case OutcomeCancelled:
		fmt.Fprintf(&b, "cancelled: %s", stopReason)
	default:
		switch {
		case exit.Err != nil:
			fmt.Fprintf(&b, "failed: %v", exit.Err)
		case exit.Signal != "":
			fmt.Fprintf(&b, "abandoned: the executor was killed by signal %s (attempt %d)", exit.Signal, s.req.Attempt)
		default:
			fmt.Fprintf(&b, "failed: executor %s exited with code %d (attempt %d)", ex.Name, exit.Code, s.req.Attempt)
		}
	}
	if len(res.Proposals) > 0 {
		ids := make([]string, len(res.Proposals))
		for i, p := range res.Proposals {
			ids[i] = p.ID
		}
		fmt.Fprintf(&b, "; proposals: %s", strings.Join(ids, ", "))
	}
	if res.Outcome != OutcomeCompleted {
		if tailTxt := strings.TrimSpace(secret.Scrub(s.stderr.Last(600), s.token)); tailTxt != "" {
			b.WriteString("; stderr: " + tailOneLine(tailTxt, 500))
		}
	}
	res.Summary = b.String()
	return res
}

// readResult reads the optional result file the executor may have written.
func (s *Session) readResult() string {
	f, err := os.Open(s.resultFile)
	if err != nil {
		return ""
	}
	defer f.Close()
	raw, err := io.ReadAll(io.LimitReader(f, 8<<10))
	if err != nil {
		return ""
	}
	var r struct {
		Summary string `json:"summary"`
	}
	if json.Unmarshal(raw, &r) != nil {
		return ""
	}
	return oneLine(secret.Scrub(strings.TrimSpace(r.Summary), s.token))
}

// tailOneLine collapses s to one line and keeps its last max bytes: the end of
// an error log is what explains the failure.
func tailOneLine(s string, max int) string {
	s = strings.Join(strings.Fields(strings.ReplaceAll(s, "\n", " | ")), " ")
	if len(s) > max {
		s = "..." + s[len(s)-max:]
	}
	return s
}

func oneLine(s string) string {
	s = strings.Join(strings.Fields(strings.ReplaceAll(s, "\n", " | ")), " ")
	if len(s) > 500 {
		s = s[:500] + "..."
	}
	return s
}
