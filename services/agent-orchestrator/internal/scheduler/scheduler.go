// Package scheduler decides which ready task to hand to which executor, and
// when. It starts sessions (internal/session) and keeps a view of tasks, runs,
// leases and proposals that it recomputes from the server on every pass.
//
// Everything here is scheduling: the scheduler cannot integrate, review or
// change policy, and its state is rebuilt from the server after a restart.
package scheduler

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/glob"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/loop"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
)

// maxTempFails bounds how often EX_TEMPFAIL (exit 75) is retried for free.
const maxTempFails = 10

// API is the read side of AGWP the scheduler needs (satisfied by *agwp.Client).
type API interface {
	ListTasks(ctx context.Context, repo, state string, readyOnly bool) ([]agwp.Task, error)
	ListRuns(ctx context.Context, repo, agent string, activeOnly bool, limit int) ([]agwp.Run, error)
	ListLeases(ctx context.Context, repo string, activeOnly bool) ([]agwp.Lease, error)
	ListProposals(ctx context.Context, repo string, f agwp.ProposalFilter) ([]agwp.Proposal, error)
	GetProposal(ctx context.Context, repo, id string) (*agwp.ProposalDetail, error)
	Predict(ctx context.Context, repo, stream string) ([]agwp.Overlap, error)
	CompleteRun(ctx context.Context, repo, runID, status, summary string) (*agwp.Run, error)
}

// Sessions starts and supervises sessions (satisfied by *session.Manager).
type Sessions interface {
	Start(ctx context.Context, req session.Request) (*session.Info, error)
	Active() []session.Info
	FreeSlots(c config.Executor) int
	Cancel(runID, reason string) bool
	Has(runID string) bool
	FinishTask(ctx context.Context, repo, taskID, state string) error
}

// PathSource tells which files each open proposal changes (satisfied by
// execsched); it may be nil.
type PathSource interface {
	OpenProposalPaths(repo string) map[string][]string
}

// Options wires a Scheduler.
type Options struct {
	Repo     config.Repo
	Execs    []config.Executor
	API      API
	Sessions Sessions
	Paths    PathSource
	Clock    clock.Clock
	Metrics  *metrics.Service
	Log      *slog.Logger
	Loop     config.Loop
}

// schedulingSource is optional for test adapters and clients predating the launch gate.
type schedulingSource interface {
	TaskScheduling(context.Context, string) (*agwp.TaskScheduling, error)
}

type route struct {
	exec  config.Executor
	title *regexp.Regexp
	scope string
}

type awaitInfo struct {
	Agent     string
	Proposals []string
	Since     time.Time
}

// Deferral explains why a ready task has not started.
type Deferral struct {
	Task   string    `json:"task"`
	Title  string    `json:"title"`
	Reason string    `json:"reason"`
	Detail string    `json:"detail,omitempty"`
	Since  time.Time `json:"since"`
}

// Scheduler runs the scheduling loop of one repository.
type Scheduler struct {
	o      Options
	log    *slog.Logger
	routes []route
	deflt  *config.Executor
	byName map[string]config.Executor
	agents map[string]config.Executor // by agent name

	kick *loop.Kicker
	run  *loop.Runner

	mu        sync.Mutex
	attempts  map[string]int
	tempFails map[string]int
	retryAt   map[string]time.Time
	awaiting  map[string]awaitInfo
	deferred  map[string]Deferral
	lastPass  time.Time
	lastErr   string
	tasksSeen map[string]int
	// seeded: tasks whose retry budget was recovered from the server's run history (after a restart)
	seeded map[string]bool
	// predictErr is why the server's conflict prediction could not be read in the last pass ("" when it could)
	predictErr  string
	paused      bool
	pauseReason string
}

// New builds a scheduler. It validates that every configured route exists.
func New(o Options) (*Scheduler, error) {
	if o.Clock == nil {
		o.Clock = clock.Real{}
	}
	if o.Metrics == nil {
		o.Metrics = metrics.NewService(metrics.NewRegistry())
	}
	if o.Log == nil {
		o.Log = slog.Default()
	}
	s := &Scheduler{o: o, log: o.Log.With("repo", o.Repo.Name, "component", "scheduler"), byName: map[string]config.Executor{}, agents: map[string]config.Executor{},
		attempts: map[string]int{}, tempFails: map[string]int{}, retryAt: map[string]time.Time{}, awaiting: map[string]awaitInfo{},
		deferred: map[string]Deferral{}, tasksSeen: map[string]int{}, seeded: map[string]bool{}, kick: loop.NewKicker()}
	for _, e := range o.Execs {
		s.byName[e.Name] = e
		s.agents[e.Agent] = e
	}
	sc := o.Repo.Schedule
	for _, r := range sc.Routes {
		e, ok := s.byName[r.Executor]
		if !ok {
			return nil, fmt.Errorf("scheduler: route to unknown executor %q", r.Executor)
		}
		rt := route{exec: e, scope: r.Scope}
		if r.Title != "" {
			re, err := regexp.Compile(r.Title)
			if err != nil {
				return nil, fmt.Errorf("scheduler: route title %q: %w", r.Title, err)
			}
			rt.title = re
		}
		s.routes = append(s.routes, rt)
	}
	if sc.Default != "" {
		e, ok := s.byName[sc.Default]
		if !ok {
			return nil, fmt.Errorf("scheduler: unknown default executor %q", sc.Default)
		}
		s.deflt = &e
	}
	s.run = &loop.Runner{Name: "scheduler", Repo: o.Repo.Name, Interval: o.Loop.Reconcile, Debounce: o.Loop.Debounce, Clock: o.Clock, Metrics: o.Metrics, Log: o.Log}
	return s, nil
}

// Kick wakes the loop (events and finished sessions call this).
func (s *Scheduler) Kick() { s.kick.Kick() }

// Run reconciles until ctx is done.
func (s *Scheduler) Run(ctx context.Context) { s.run.Run(ctx, s.kick, s.Reconcile) }

// Runs is the number of completed passes.
func (s *Scheduler) Runs() uint64 { return s.run.Runs() }

// routeFor picks the executor for a task: the first matching route, else the default.
func (s *Scheduler) routeFor(t agwp.Task) (config.Executor, bool) {
	for _, r := range s.routes {
		if r.title != nil && !r.title.MatchString(t.Title) {
			continue
		}
		if r.scope != "" && !glob.AnyOverlap(t.Scope, []string{r.scope}) {
			continue
		}
		return r.exec, true
	}
	if s.deflt != nil {
		return *s.deflt, true
	}
	return config.Executor{}, false
}

func (s *Scheduler) managedAgent(identity string) bool {
	name := identity
	if len(name) > 6 && name[:6] == "agent:" {
		name = name[6:]
	}
	_, ok := s.agents[name]
	return ok
}

// Reconcile is one scheduling pass. It is safe to call concurrently with
// session callbacks and from tests.
func (s *Scheduler) Reconcile(ctx context.Context) (err error) {
	repo := s.o.Repo.Name
	defer func() {
		s.mu.Lock()
		s.lastPass = s.o.Clock.Now()
		s.lastErr = ""
		if err != nil && ctx.Err() == nil { // an error caused by shutdown is not news
			s.lastErr = err.Error()
		}
		s.mu.Unlock()
	}()
	tasks, err := s.o.API.ListTasks(ctx, repo, "", false)
	if err != nil {
		return fmt.Errorf("list tasks: %w", err)
	}
	runs, err := s.o.API.ListRuns(ctx, repo, "", true, 1000)
	if err != nil {
		return fmt.Errorf("list runs: %w", err)
	}
	leases, err := s.o.API.ListLeases(ctx, repo, true)
	if err != nil {
		return fmt.Errorf("list leases: %w", err)
	}
	open, err := s.o.API.ListProposals(ctx, repo, agwp.ProposalFilter{State: agwp.ProposalOpen})
	if err != nil {
		return fmt.Errorf("list proposals: %w", err)
	}

	s.reapOrphans(ctx, runs)
	activeRunTask := map[string]bool{} // tasks with a live run (ours or anyone's)
	for _, r := range runs {
		if r.TaskID != "" {
			activeRunTask[r.TaskID] = true
		}
	}
	byID := map[string]agwp.Task{}
	for _, t := range tasks {
		byID[t.ID] = t
	}
	s.cancelStoppedWork(byID)
	s.resolveWaiting(ctx, tasks, activeRunTask, open)
	s.countTasks(tasks)

	if source, ok := s.o.API.(schedulingSource); ok {
		policy, err := source.TaskScheduling(ctx, repo)
		if err != nil {
			return fmt.Errorf("read task scheduling: %w", err)
		}
		s.mu.Lock()
		s.paused, s.pauseReason = policy.Paused, policy.Reason
		s.mu.Unlock()
		if policy.Paused {
			for _, task := range tasks {
				if task.State == agwp.TaskOpen && task.Ready {
					s.setDeferred(task, ReasonOperatorPause, policy.Reason)
				}
			}
			return nil
		}
	}

	running := s.repoSessions()
	slots := s.o.Repo.Schedule.Slots
	free := slots - len(running)
	cands := s.candidates(tasks, activeRunTask, open)
	if free <= 0 {
		for _, t := range cands {
			if _, ok := s.executorFor(t); ok {
				s.setDeferred(t, ReasonRepoBusy, fmt.Sprintf("the repository's limit of %d sessions is reached", slots))
			}
		}
		return nil
	}

	overlaps, perr := s.o.API.Predict(ctx, repo, s.o.Repo.Stream)
	s.notePredict(perr)
	if perr != nil {
		overlaps = nil
	}
	holdForPrediction := perr != nil && s.o.Repo.Schedule.OnPredictError == "defer" && s.o.Repo.Schedule.Placement == "avoid"
	var paths map[string][]string
	if s.o.Paths != nil {
		paths = s.o.Paths.OpenProposalPaths(repo)
	}

	now := s.o.Clock.Now()
	seen := map[string]bool{}
	for _, t := range cands {
		seen[t.ID] = true
		if free <= 0 {
			s.setDeferred(t, ReasonRepoBusy, fmt.Sprintf("%d of %d sessions running", len(running), slots))
			continue
		}
		ex, ok := s.executorFor(t)
		if !ok {
			continue // not ours
		}
		if t.State == agwp.TaskInProgress {
			// An adopted task: the previous process may have spent attempts on it.
			if !s.recoverBudget(ctx, t, ex) {
				continue
			}
		}
		s.mu.Lock()
		retry := s.retryAt[t.ID]
		s.mu.Unlock()
		if retry.After(now) {
			s.setDeferred(t, ReasonRetryBackoff, "next attempt at "+retry.Format(time.RFC3339))
			continue
		}
		if s.o.Sessions.FreeSlots(ex) <= 0 {
			s.setDeferred(t, ReasonExecutorBusy, "executor "+ex.Name+" is at its session limit")
			continue
		}
		if holdForPrediction {
			s.setDeferred(t, ReasonPredictUnavailable, "the server's conflict prediction cannot be read ("+perr.Error()+"); on_predict_error is defer")
			continue
		}
		dec := Decide(Input{
			Task: t, Policy: s.o.Repo.Schedule.Placement, Agent: ex.Agent, Leases: leases, Tasks: tasks, Running: running,
			Awaiting: s.awaitingTasks(byID), OpenProposals: open, ProposalPaths: paths, Overlaps: overlaps,
		})
		if dec.Defer {
			s.setDeferred(t, dec.Reason, dec.Detail)
			continue
		}
		started, serr := s.startSession(ctx, t, ex)
		switch {
		case serr == nil:
			free--
			running = append(running, *started)
			s.clearDeferred(t.ID)
		case errors.Is(serr, session.ErrClosed):
			return nil
		case errors.Is(serr, session.ErrNoSlot):
			s.setDeferred(t, ReasonExecutorBusy, "executor "+ex.Name+" is at its session limit")
		case errors.Is(serr, session.ErrLeaseConflict):
			s.setDeferred(t, ReasonLeaseConflict, serr.Error())
		case errors.Is(serr, session.ErrTaskUnavailable):
			s.log.Info("the server will not start this task; leaving it alone", "task", t.ID, "err", serr)
		default:
			s.log.Warn("could not start a session", "task", t.ID, "executor", ex.Name, "err", serr)
			s.mu.Lock()
			s.retryAt[t.ID] = now.Add(s.backoffFor(1))
			s.mu.Unlock()
			s.setDeferred(t, ReasonStartFailed, serr.Error())
		}
	}
	s.pruneDeferred(seen)
	return nil
}

// notePredict records whether the server's conflict prediction could be read in this pass. A failure is never
// silent: it is logged once per outage, counted on every pass, and shown in /status.
func (s *Scheduler) notePredict(err error) {
	s.mu.Lock()
	prev := s.predictErr
	s.predictErr = ""
	if err != nil {
		s.predictErr = err.Error()
	}
	s.mu.Unlock()
	switch {
	case err != nil:
		s.o.Metrics.PredictErrors.With(s.o.Repo.Name).Inc()
		if prev == "" {
			s.log.Warn("conflict prediction unavailable; placement does not see the server's overlap prediction until it is back", "err", err, "on_predict_error", s.o.Repo.Schedule.OnPredictError)
		}
	case prev != "":
		s.log.Info("conflict prediction available again")
	}
}

// recoverBudget restores, for a task this process did not start, the attempts that earlier runs of it already
// spent. The run history is on the server; the counters of a previous orchestrator process are gone. Without this
// a restart would hand a task that failed `attempts` times another full budget, and a crash-looping executor
// would be retried without end. It reports whether the task may be started now.
func (s *Scheduler) recoverBudget(ctx context.Context, t agwp.Task, ex config.Executor) bool {
	repo := s.o.Repo.Name
	s.mu.Lock()
	_, retrying := s.retryAt[t.ID]
	known := s.seeded[t.ID] || s.attempts[t.ID] > 0 || retrying
	s.mu.Unlock()
	if known {
		return true
	}
	runs, err := s.o.API.ListRuns(ctx, repo, ex.Agent, false, 200)
	if err != nil {
		s.setDeferred(t, ReasonStartFailed, "could not read the run history of the task: "+err.Error())
		return false
	}
	spent, temp := 0, 0
	var last time.Time
	lastTemp := false
	for _, r := range runs {
		if r.TaskID != t.ID {
			continue
		}
		charge := session.ChargeOf(r.Status, r.Summary)
		if charge == session.ChargeNone {
			continue
		}
		if charge == session.ChargeAttempt {
			spent++
		} else {
			temp++
		}
		if fin := time.UnixMilli(r.FinishedAtMs); fin.After(last) {
			last, lastTemp = fin, charge == session.ChargeTemporary
		}
	}
	s.mu.Lock()
	s.seeded[t.ID] = true
	s.mu.Unlock()
	if spent+temp == 0 {
		return true
	}
	if spent >= s.o.Repo.Schedule.Attempts || temp >= maxTempFails {
		s.log.Warn("giving up on the task: its attempt budget was spent before this process started", "task", t.ID, "attempts", spent, "temporary_failures", temp)
		if err := s.o.Sessions.FinishTask(ctx, repo, t.ID, agwp.TaskCancelled); err != nil && !agwp.IsCode(err, agwp.CodeFailedPrecond) {
			s.log.Warn("could not cancel the task", "task", t.ID, "err", err)
		}
		s.clearDeferred(t.ID)
		return false
	}
	n := spent
	if lastTemp {
		n = temp
	}
	s.mu.Lock()
	s.attempts[t.ID], s.tempFails[t.ID] = spent, temp
	s.retryAt[t.ID] = last.Add(s.backoffFor(n))
	s.mu.Unlock()
	s.log.Info("recovered the retry budget of a task from its run history", "task", t.ID, "attempts", spent, "temporary_failures", temp, "retry_at", last.Add(s.backoffFor(n)))
	return true
}

// executorFor respects an explicit assignment before considering routing rules.
// An agent this process does not supervise is left to its own orchestrator.
func (s *Scheduler) executorFor(t agwp.Task) (config.Executor, bool) {
	if t.Owner != "" {
		if name, ok := strings.CutPrefix(t.Owner, "agent:"); ok {
			if e, ok := s.agents[name]; ok {
				return e, true
			}
		}
		return config.Executor{}, false
	}
	return s.routeFor(t)
}

// candidates are the tasks that could start now, oldest first: open tasks whose
// dependencies are done, and in-progress tasks of our agents that nothing is
// working on and that produced nothing (a crashed or interrupted session).
func (s *Scheduler) candidates(tasks []agwp.Task, liveRun map[string]bool, open []agwp.Proposal) []agwp.Task {
	hasOpen := map[string]bool{}
	for _, p := range open {
		if p.TaskID != "" {
			hasOpen[p.TaskID] = true
		}
	}
	var out []agwp.Task
	for _, t := range tasks {
		switch {
		case t.State == agwp.TaskOpen && t.Ready:
			out = append(out, t)
		case t.State == agwp.TaskInProgress && s.managedAgent(t.Owner) && !liveRun[t.ID] && !hasOpen[t.ID] && !s.isAwaiting(t.ID) && !s.hasHistory(t.ID):
			out = append(out, t)
		}
	}
	sort.SliceStable(out, func(i, j int) bool {
		if out[i].CreatedAtMs != out[j].CreatedAtMs {
			return out[i].CreatedAtMs < out[j].CreatedAtMs
		}
		return out[i].ID < out[j].ID
	})
	return out
}

func (s *Scheduler) isAwaiting(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	_, ok := s.awaiting[id]
	return ok
}

// hasHistory: an in-progress task that already ran in this process is only
// restarted through the retry bookkeeping, never adopted a second time.
func (s *Scheduler) hasHistory(id string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	_, retrying := s.retryAt[id]
	return s.attempts[id] > 0 && !retrying
}

func (s *Scheduler) awaitingTasks(byID map[string]agwp.Task) []agwp.Task {
	s.mu.Lock()
	defer s.mu.Unlock()
	var out []agwp.Task
	for id := range s.awaiting {
		if t, ok := byID[id]; ok {
			out = append(out, t)
		}
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

func (s *Scheduler) repoSessions() []session.Info {
	var out []session.Info
	for _, i := range s.o.Sessions.Active() {
		if i.Repo == s.o.Repo.Name {
			out = append(out, i)
		}
	}
	return out
}

func (s *Scheduler) startSession(ctx context.Context, t agwp.Task, ex config.Executor) (*session.Info, error) {
	s.mu.Lock()
	attempt := s.attempts[t.ID] + 1
	s.mu.Unlock()
	sc := s.o.Repo.Schedule
	info, err := s.o.Sessions.Start(ctx, session.Request{
		Repo: s.o.Repo.Name, Stream: s.o.Repo.Stream, Task: t, Executor: ex, Attempt: attempt,
		Lease: sc.Lease, LeaseTTL: sc.LeaseTTL, Decide: s.decide,
	})
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	s.attempts[t.ID] = attempt
	delete(s.retryAt, t.ID)
	s.mu.Unlock()
	return info, nil
}

// decide is called by a session when it ended, before its credential is
// revoked. It chooses the task's fate and records what the scheduler must
// remember (retries, work waiting to land).
func (s *Scheduler) decide(res session.Result) session.Disposition {
	defer s.kick.Kick()
	s.mu.Lock()
	defer s.mu.Unlock()
	id := res.Task.ID
	open := 0
	var props []string
	for _, p := range res.Proposals {
		props = append(props, p.ID)
		if p.State == agwp.ProposalOpen {
			open++
		}
	}
	waitForLanding := func() session.Disposition {
		s.awaiting[id] = awaitInfo{Agent: res.Agent, Proposals: props, Since: s.o.Clock.Now()}
		return session.Keep
	}
	switch res.Outcome {
	case session.OutcomeCompleted:
		if len(props) > 0 {
			return waitForLanding()
		}
		return session.Done
	case session.OutcomeCancelled:
		return session.Keep // shutdown, or someone else ended the task: nothing to decide
	}
	// failed or timed out
	if open > 0 {
		s.log.Warn("the session failed after opening proposals; the task waits for them", "task", id, "proposals", props)
		return waitForLanding()
	}
	if res.ExitCode == session.ExitTempFail && s.tempFails[id] < maxTempFails {
		s.tempFails[id]++
		s.attempts[id]-- // a temporary failure does not spend the attempt budget
		s.retryAt[id] = s.o.Clock.Now().Add(s.backoffFor(s.tempFails[id]))
		return session.Keep
	}
	if res.Attempt < s.o.Repo.Schedule.Attempts {
		s.retryAt[id] = s.o.Clock.Now().Add(s.backoffFor(res.Attempt))
		s.log.Info("the session failed; the task will be retried", "task", id, "attempt", res.Attempt, "of", s.o.Repo.Schedule.Attempts, "retry_at", s.retryAt[id])
		return session.Keep
	}
	s.log.Warn("giving up on the task", "task", id, "attempts", res.Attempt, "outcome", res.Outcome)
	return session.Cancel
}

// backoffFor is the delay before retry n (1-based): backoff * 2^(n-1), capped at 1h.
func (s *Scheduler) backoffFor(n int) time.Duration {
	d := s.o.Repo.Schedule.Backoff
	for i := 1; i < n && d < time.Hour; i++ {
		d *= 2
	}
	return min(d, time.Hour)
}

// reapOrphans completes runs of our agents that no session of ours supervises
// and that stopped heartbeating: leftovers of a crashed orchestrator or of an
// executor start whose outcome was unknown.
func (s *Scheduler) reapOrphans(ctx context.Context, runs []agwp.Run) {
	now := s.o.Clock.Now()
	for _, r := range runs {
		if r.Status != "running" || !s.managedAgent("agent:"+r.Agent) || s.o.Sessions.Has(r.RunID) {
			continue
		}
		ex := s.agents[r.Agent]
		stale := max(3*ex.Beat, time.Minute)
		last := time.UnixMilli(max(r.LastHeartbeatMs, r.StartedAtMs))
		if now.Sub(last) < stale {
			continue
		}
		_, err := s.o.API.CompleteRun(ctx, s.o.Repo.Name, r.RunID, "failed",
			fmt.Sprintf(session.LostRunPrefix+" for %s and no session of this orchestrator supervises the run (orchestrator restarted, or the executor start was interrupted)", now.Sub(last).Round(time.Second)))
		if err != nil && !agwp.IsNotFound(err) {
			s.log.Warn("could not complete an orphaned run", "run", r.RunID, "err", err)
			continue
		}
		s.log.Warn("completed an orphaned run", "run", r.RunID, "agent", r.Agent, "task", r.TaskID)
		s.kick.Kick() // the task can be adopted on the next pass
		s.o.Metrics.SessionsFinished.With(s.o.Repo.Name, ex.Name, "lost").Inc()
	}
}

// cancelStoppedWork stops sessions whose task somebody finished or cancelled.
func (s *Scheduler) cancelStoppedWork(byID map[string]agwp.Task) {
	for _, i := range s.repoSessions() {
		t, ok := byID[i.TaskID]
		if ok && (t.State == agwp.TaskDone || t.State == agwp.TaskCancelled) && i.State == "running" {
			s.log.Info("the task was ended by someone else; stopping its session", "task", t.ID, "state", t.State, "run", i.RunID)
			s.o.Sessions.Cancel(i.RunID, "task "+t.State+" by someone else")
		}
	}
}

// resolveWaiting decides the fate of tasks whose work was proposed: done once
// a proposal landed, cancelled when every proposal was closed unmerged. After a
// restart it also rediscovers such tasks from the proposals' task ids.
func (s *Scheduler) resolveWaiting(ctx context.Context, tasks []agwp.Task, liveRun map[string]bool, open []agwp.Proposal) {
	repo := s.o.Repo.Name
	// rediscover after a restart
	var limbo []agwp.Task
	for _, t := range tasks {
		if t.State == agwp.TaskInProgress && s.managedAgent(t.Owner) && !liveRun[t.ID] && !s.isAwaiting(t.ID) && s.attemptsOf(t.ID) == 0 {
			limbo = append(limbo, t)
		}
	}
	if len(limbo) > 0 {
		all, err := s.o.API.ListProposals(ctx, repo, agwp.ProposalFilter{})
		if err == nil {
			byTask := map[string][]string{}
			for _, p := range all {
				if p.TaskID != "" {
					byTask[p.TaskID] = append(byTask[p.TaskID], p.ID)
				}
			}
			s.mu.Lock()
			for _, t := range limbo {
				if ids := byTask[t.ID]; len(ids) > 0 {
					s.awaiting[t.ID] = awaitInfo{Agent: t.Owner[6:], Proposals: ids, Since: s.o.Clock.Now()}
				}
			}
			s.mu.Unlock()
		}
	}
	s.mu.Lock()
	pending := map[string]awaitInfo{}
	for id, a := range s.awaiting {
		pending[id] = a
	}
	s.mu.Unlock()
	byID := map[string]agwp.Task{}
	for _, t := range tasks {
		byID[t.ID] = t
	}
	for id, a := range pending {
		t, ok := byID[id]
		if !ok || t.State == agwp.TaskDone || t.State == agwp.TaskCancelled {
			s.forgetWaiting(id)
			continue
		}
		integrated, openNow := 0, 0
		for _, pid := range a.Proposals {
			d, err := s.o.API.GetProposal(ctx, repo, pid)
			if err != nil {
				openNow++ // cannot tell; wait
				continue
			}
			switch d.Proposal.State {
			case agwp.ProposalIntegrated:
				integrated++
			case agwp.ProposalOpen:
				openNow++
			}
		}
		// proposals opened later by the same task (after an update) are found through the open list
		for _, p := range open {
			if p.TaskID == id && !contains(a.Proposals, p.ID) {
				openNow++
			}
		}
		var state string
		switch {
		case openNow > 0:
			continue
		case integrated > 0:
			state = agwp.TaskDone
		default:
			state = agwp.TaskCancelled
		}
		if err := s.o.Sessions.FinishTask(ctx, repo, id, state); err != nil && !agwp.IsCode(err, agwp.CodeFailedPrecond) {
			s.log.Warn("could not finish the task", "task", id, "state", state, "err", err)
			continue
		}
		s.log.Info("work landed or was closed; task finished", "task", id, "state", state)
		s.forgetWaiting(id)
		// The caller's task list was read before this; record the new state in it so the
		// finished task is not offered for adoption in this same pass.
		for i := range tasks {
			if tasks[i].ID == id {
				tasks[i].State = state
			}
		}
	}
}

func (s *Scheduler) attemptsOf(id string) int {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.attempts[id]
}

func (s *Scheduler) forgetWaiting(id string) {
	s.mu.Lock()
	delete(s.awaiting, id)
	delete(s.attempts, id)
	delete(s.tempFails, id)
	delete(s.retryAt, id)
	delete(s.seeded, id)
	s.mu.Unlock()
	s.clearDeferred(id)
}

func contains(list []string, s string) bool {
	for _, x := range list {
		if x == s {
			return true
		}
	}
	return false
}

func (s *Scheduler) countTasks(tasks []agwp.Task) {
	counts := map[string]int{}
	for _, t := range tasks {
		counts[t.State]++
		if t.State == agwp.TaskOpen && t.Ready {
			counts["ready"]++
		}
	}
	s.mu.Lock()
	s.tasksSeen = counts
	s.mu.Unlock()
}

func (s *Scheduler) setDeferred(t agwp.Task, reason, detail string) {
	s.mu.Lock()
	prev, had := s.deferred[t.ID]
	since := s.o.Clock.Now()
	if had && prev.Reason == reason {
		since = prev.Since
	}
	s.deferred[t.ID] = Deferral{Task: t.ID, Title: t.Title, Reason: reason, Detail: detail, Since: since}
	s.mu.Unlock()
	if !had || prev.Reason != reason {
		s.o.Metrics.TasksDeferred.With(s.o.Repo.Name, reason).Inc()
		s.log.Info("task deferred", "task", t.ID, "reason", reason, "detail", detail)
	}
}

func (s *Scheduler) clearDeferred(id string) {
	s.mu.Lock()
	delete(s.deferred, id)
	s.mu.Unlock()
}

func (s *Scheduler) pruneDeferred(seen map[string]bool) {
	s.mu.Lock()
	for id := range s.deferred {
		if !seen[id] {
			delete(s.deferred, id)
		}
	}
	s.mu.Unlock()
}

// Status is the scheduler's part of /status.
type Status struct {
	Paused      bool           `json:"paused"`
	PauseReason string         `json:"pause_reason,omitempty"`
	Repo        string         `json:"repo"`
	Slots       int            `json:"slots"`
	Running     []session.Info `json:"running"`
	Waiting     []Waiting      `json:"waiting_to_land"`
	Deferred    []Deferral     `json:"deferred"`
	Retrying    []Retry        `json:"retrying"`
	Tasks       map[string]int `json:"tasks"`
	LastPass    time.Time      `json:"last_pass,omitempty"`
	LastError   string         `json:"last_error,omitempty"`
	// PredictError is why the server's conflict prediction could not be read in the last pass; empty when it could.
	PredictError string `json:"predict_error,omitempty"`
}

// Waiting is a task whose proposals have not landed yet.
type Waiting struct {
	Task      string    `json:"task"`
	Agent     string    `json:"agent"`
	Proposals []string  `json:"proposals"`
	Since     time.Time `json:"since"`
}

// Retry is a task waiting for its next attempt.
type Retry struct {
	Task    string    `json:"task"`
	At      time.Time `json:"at"`
	Attempt int       `json:"attempts_made"`
}

// Status returns a snapshot for the admin API.
func (s *Scheduler) Status() Status {
	// lists are never null in the JSON
	st := Status{Repo: s.o.Repo.Name, Slots: s.o.Repo.Schedule.Slots, Running: s.repoSessions(),
		Waiting: []Waiting{}, Deferred: []Deferral{}, Retrying: []Retry{}}
	if st.Running == nil {
		st.Running = []session.Info{}
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	st.Paused, st.PauseReason = s.paused, s.pauseReason
	st.LastPass, st.LastError, st.PredictError = s.lastPass, s.lastErr, s.predictErr
	st.Tasks = map[string]int{}
	for k, v := range s.tasksSeen {
		st.Tasks[k] = v
	}
	for id, a := range s.awaiting {
		st.Waiting = append(st.Waiting, Waiting{Task: id, Agent: a.Agent, Proposals: a.Proposals, Since: a.Since})
	}
	for _, d := range s.deferred {
		st.Deferred = append(st.Deferred, d)
	}
	for id, at := range s.retryAt {
		st.Retrying = append(st.Retrying, Retry{Task: id, At: at, Attempt: s.attempts[id]})
	}
	sort.Slice(st.Waiting, func(i, j int) bool { return st.Waiting[i].Task < st.Waiting[j].Task })
	sort.Slice(st.Deferred, func(i, j int) bool { return st.Deferred[i].Task < st.Deferred[j].Task })
	sort.Slice(st.Retrying, func(i, j int) bool { return st.Retrying[i].Task < st.Retrying[j].Task })
	return st
}
