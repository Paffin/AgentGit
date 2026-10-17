// Package execsched is the execution scheduler: it keeps every open proposal
// "prepared" so that the checks its policy requires are queued for the trusted
// execution plane, and it reports where each proposal stands on the four
// independent questions of AgentGit:
//
//	mergeable   is there one unambiguous result tree?        (the merge engine)
//	validated   did trusted checks pass on that exact tree?  (signed attestations)
//	approved    did enough eligible people approve the head? (reviews + policy)
//	integrated  was the stream updated atomically?           (the stream's CAS)
//
// The package only calls integration.prepare, which never changes a stream.
// It does not run checks, sign attestations or integrate: queued jobs are
// claimed by runner-role workers whose keys the policy trusts. A required
// check that the policy gives no way to run stays "missing" and is reported as
// such.
package execsched

import (
	"context"
	"fmt"
	"log/slog"
	"sort"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/loop"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

// API is what the execution scheduler calls (satisfied by *agwp.Client).
type API interface {
	ListProposals(ctx context.Context, repo string, f agwp.ProposalFilter) ([]agwp.Proposal, error)
	Prepare(ctx context.Context, repo, proposal string, enqueue bool) (*agwp.Preparation, error)
	ReadHead(ctx context.Context, repo, stream string) (*agwp.Stream, error)
	ListJobs(ctx context.Context, repo, state string, limit int) ([]agwp.Job, error)
	ListQueue(ctx context.Context, repo, stream string) ([]agwp.QueueEntry, error)
}

// Answers for the independent states.
const (
	Yes     = "yes"
	No      = "no"
	Pending = "pending"
	NA      = "na"
	Unknown = "unknown"
)

// Blocker is a stable-coded reason the proposal cannot land yet.
type Blocker struct {
	Code    string `json:"code"`
	Message string `json:"message"`
}

// ProposalStatus is the per-proposal view served at /status.
type ProposalStatus struct {
	ID     string `json:"id"`
	Title  string `json:"title"`
	Author string `json:"author"`
	Stream string `json:"stream"`
	Head   string `json:"head"`
	TaskID string `json:"task_id,omitempty"`

	Mergeable  string `json:"mergeable"`
	Validated  string `json:"validated"`
	Approved   string `json:"approved"`
	Integrated bool   `json:"integrated"`
	// Ready is true when nothing but a person's decision to integrate is
	// missing, as far as integration.prepare can tell for the orchestrator's identity.
	Ready bool `json:"ready_to_integrate"`

	Conflicts     []agwp.Conflict         `json:"conflicts,omitempty"`
	Problem       string                  `json:"problem,omitempty"`
	ChangedFiles  []string                `json:"changed_files,omitempty"`
	Blockers      []Blocker               `json:"blockers,omitempty"`
	Validations   []agwp.ValidationStatus `json:"validations,omitempty"`
	Jobs          []JobRef                `json:"jobs,omitempty"`
	Strategy      string                  `json:"strategy,omitempty"`
	CandidateTree string                  `json:"candidate_tree,omitempty"`
	Base          string                  `json:"base,omitempty"`
	PolicyVersion uint64                  `json:"policy_version"`
	PreparedAt    time.Time               `json:"prepared_at,omitempty"`
	StreamHead    string                  `json:"prepared_against,omitempty"`
	Error         string                  `json:"error,omitempty"`
}

// JobRef is a queued validation job.
type JobRef struct {
	ID    string `json:"id"`
	Check string `json:"check"`
	State string `json:"state"`
}

// Prepared is delivered to listeners whenever a proposal was (re)prepared.
type Prepared struct {
	Repo       string
	Proposal   agwp.Proposal
	Prep       agwp.Preparation
	StreamHead string
}

// Status is the execution scheduler's part of /status.
type Status struct {
	Repo          string            `json:"repo"`
	Stream        string            `json:"stream"`
	StreamHead    string            `json:"stream_head,omitempty"`
	StreamVersion uint64            `json:"stream_version"`
	Open          []ProposalStatus  `json:"open"`
	Integrated    []ProposalStatus  `json:"recently_integrated,omitempty"`
	Jobs          []agwp.Job        `json:"validation_queue,omitempty"`
	Queue         []agwp.QueueEntry `json:"integration_queue,omitempty"`
	LastPass      time.Time         `json:"last_pass,omitempty"`
	LastError     string            `json:"last_error,omitempty"`
}

// Options wires an execution scheduler.
type Options struct {
	Repo    config.Repo
	API     API
	Clock   clock.Clock
	Metrics *metrics.Service
	Log     *slog.Logger
	Loop    config.Loop
	// Refresh is how often a proposal that is not fully green is prepared
	// again even when nothing is known to have changed (default 15s).
	Refresh time.Duration
	// Recent bounds the remembered integrated proposals (default 20).
	Recent int
}

type entry struct {
	status    ProposalStatus
	key       string
	settled   bool
	preparedT time.Time
	seenJobs  map[string]bool
}

// Scheduler is the execution scheduler of one repository.
type Scheduler struct {
	o    Options
	log  *slog.Logger
	kick *loop.Kicker
	run  *loop.Runner

	mu         sync.Mutex
	entries    map[string]*entry // open proposals by id
	recent     []ProposalStatus  // integrated, newest first
	dirty      map[string]bool   // proposals to re-prepare regardless of timers
	dirtyAll   bool
	head       agwp.Stream
	jobs       []agwp.Job
	queue      []agwp.QueueEntry
	lastPass   time.Time
	lastErr    string
	listeners  []func(Prepared)
	forgetters []func(string)
}

// New builds the scheduler.
func New(o Options) *Scheduler {
	if o.Clock == nil {
		o.Clock = clock.Real{}
	}
	if o.Metrics == nil {
		o.Metrics = metrics.NewService(metrics.NewRegistry())
	}
	if o.Log == nil {
		o.Log = slog.Default()
	}
	if o.Refresh <= 0 {
		o.Refresh = 15 * time.Second
	}
	if o.Recent <= 0 {
		o.Recent = 20
	}
	s := &Scheduler{o: o, log: o.Log.With("repo", o.Repo.Name, "component", "execsched"), kick: loop.NewKicker(),
		entries: map[string]*entry{}, dirty: map[string]bool{}}
	s.run = &loop.Runner{Name: "execsched", Repo: o.Repo.Name, Interval: o.Loop.Reconcile, Debounce: o.Loop.Debounce, Clock: o.Clock, Metrics: o.Metrics, Log: o.Log}
	return s
}

// Topics are the event prefixes that matter here.
func Topics() []string {
	return []string{"proposal", "stream", "attestation", "job", "policy", "changeset"}
}

// OnEvent turns an event into a re-preparation hint and wakes the loop.
func (s *Scheduler) OnEvent(ev agwp.Event) {
	s.mu.Lock()
	switch ev.Topic {
	case "proposal.review", "proposal.update", "proposal.create":
		s.dirty[ev.Payload.Target] = true
	case "policy.set", "attestation.submit", "stream.integrate", "stream.cas":
		s.dirtyAll = true
	default:
		if len(ev.Topic) >= 3 && ev.Topic[:3] == "job" {
			s.dirtyAll = true
		}
	}
	s.mu.Unlock()
	s.kick.Kick()
}

// Subscribe registers a listener called after each successful preparation.
// Listeners run on the reconcile goroutine and must not block.
func (s *Scheduler) Subscribe(fn func(Prepared)) {
	s.mu.Lock()
	s.listeners = append(s.listeners, fn)
	s.mu.Unlock()
}

// Run reconciles until ctx is done.
func (s *Scheduler) Run(ctx context.Context) { s.run.Run(ctx, s.kick, s.Reconcile) }

// Kick wakes the loop.
func (s *Scheduler) Kick() { s.kick.Kick() }

// Runs is the number of completed passes.
func (s *Scheduler) Runs() uint64 { return s.run.Runs() }

// OpenProposalPaths implements scheduler.PathSource: the files each open
// proposal changes, from its last preparation.
func (s *Scheduler) OpenProposalPaths(string) map[string][]string {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make(map[string][]string, len(s.entries))
	for id, e := range s.entries {
		out[id] = e.status.ChangedFiles
	}
	return out
}

// Latest returns the most recent preparation of an open proposal.
func (s *Scheduler) Latest(proposal string) (ProposalStatus, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	e, ok := s.entries[proposal]
	if !ok {
		return ProposalStatus{}, false
	}
	return e.status, true
}

// Reconcile is one pass.
func (s *Scheduler) Reconcile(ctx context.Context) (err error) {
	repo, stream := s.o.Repo.Name, s.o.Repo.Stream
	defer func() {
		s.mu.Lock()
		s.lastPass, s.lastErr = s.o.Clock.Now(), ""
		if err != nil && ctx.Err() == nil {
			s.lastErr = err.Error()
		}
		s.mu.Unlock()
	}()
	head, err := s.o.API.ReadHead(ctx, repo, stream)
	if err != nil {
		return fmt.Errorf("read stream head: %w", err)
	}
	open, err := s.o.API.ListProposals(ctx, repo, agwp.ProposalFilter{State: agwp.ProposalOpen, Stream: stream})
	if err != nil {
		return fmt.Errorf("list proposals: %w", err)
	}
	s.mu.Lock()
	s.head = *head
	dirtyAll := s.dirtyAll
	dirty := s.dirty
	s.dirtyAll, s.dirty = false, map[string]bool{}
	s.mu.Unlock()

	now := s.o.Clock.Now()
	seen := map[string]bool{}
	var firstErr error
	for _, p := range open {
		seen[p.ID] = true
		s.mu.Lock()
		e := s.entries[p.ID]
		s.mu.Unlock()
		key := fmt.Sprintf("%s@%s#%d", p.Head, head.Head, head.Version)
		need := e == nil || e.key != key || dirty[p.ID] || (dirtyAll && !e.settled) ||
			(!e.settled && now.Sub(e.preparedT) >= s.o.Refresh)
		if !need {
			continue
		}
		if perr := s.prepare(ctx, p, key, head, e); perr != nil && firstErr == nil {
			firstErr = perr
		}
	}
	s.forgetClosed(ctx, seen)
	if jobs, jerr := s.o.API.ListJobs(ctx, repo, "", 200); jerr == nil {
		s.mu.Lock()
		s.jobs = unfinished(jobs)
		s.mu.Unlock()
	}
	if q, qerr := s.o.API.ListQueue(ctx, repo, stream); qerr == nil {
		s.mu.Lock()
		s.queue = q
		s.mu.Unlock()
	}
	return firstErr
}

func unfinished(jobs []agwp.Job) []agwp.Job {
	var out []agwp.Job
	for _, j := range jobs {
		if j.State == agwp.JobQueued || j.State == agwp.JobClaimed {
			out = append(out, j)
		}
	}
	return out
}

func (s *Scheduler) prepare(ctx context.Context, p agwp.Proposal, key string, head *agwp.Stream, prev *entry) error {
	repo := s.o.Repo.Name
	enqueue := config.On(s.o.Repo.ExecSched.Enqueue)
	prep, err := s.o.API.Prepare(ctx, repo, p.ID, enqueue)
	s.o.Metrics.Preparations.With(repo).Inc()
	if err != nil {
		if agwp.IsNotFound(err) {
			return nil // closed or removed while we looked
		}
		s.mu.Lock()
		e := prev
		if e == nil {
			e = &entry{status: skeleton(p), seenJobs: map[string]bool{}}
			s.entries[p.ID] = e
		}
		e.status.Error = err.Error()
		e.status.Mergeable, e.status.Validated, e.status.Approved = unknownIfEmpty(e.status.Mergeable), unknownIfEmpty(e.status.Validated), unknownIfEmpty(e.status.Approved)
		e.preparedT = s.o.Clock.Now() // do not hammer a failing proposal
		e.key = key
		s.mu.Unlock()
		return fmt.Errorf("prepare %s: %w", p.ID, err)
	}
	st := classify(p, prep)
	st.PreparedAt, st.StreamHead = s.o.Clock.Now(), head.Head
	s.mu.Lock()
	e := prev
	if e == nil {
		e = &entry{seenJobs: map[string]bool{}}
		s.entries[p.ID] = e
	}
	newJobs := 0
	for _, j := range prep.Jobs {
		if !e.seenJobs[j.ID] {
			e.seenJobs[j.ID] = true
			newJobs++
		}
	}
	e.status, e.key, e.preparedT = st, key, st.PreparedAt
	e.settled = st.Mergeable == Yes && (st.Validated == Yes || st.Validated == NA) && st.Approved == Yes
	listeners := append([]func(Prepared){}, s.listeners...)
	s.mu.Unlock()
	if newJobs > 0 {
		s.o.Metrics.ValidationsQueued.With(repo).Add(float64(newJobs))
		s.log.Info("validation jobs queued", "proposal", p.ID, "jobs", newJobs)
	}
	for _, fn := range listeners {
		fn(Prepared{Repo: repo, Proposal: p, Prep: *prep, StreamHead: head.Head})
	}
	return nil
}

func unknownIfEmpty(s string) string {
	if s == "" {
		return Unknown
	}
	return s
}

func skeleton(p agwp.Proposal) ProposalStatus {
	return ProposalStatus{ID: p.ID, Title: p.Title, Author: p.Author, Stream: p.Stream, Head: p.Head, TaskID: p.TaskID,
		Mergeable: Unknown, Validated: Unknown, Approved: Unknown}
}

// classify derives the four states from one preparation. Tolerates an empty
// job list: validations are then reported as missing without a job.
func classify(p agwp.Proposal, prep *agwp.Preparation) ProposalStatus {
	st := skeleton(p)
	st.Conflicts, st.Problem, st.ChangedFiles = prep.Conflicts, prep.Problem, prep.ChangedFiles
	st.Validations, st.Strategy, st.CandidateTree, st.Base, st.PolicyVersion = prep.Validations, prep.Strategy, prep.CandidateTree, prep.Base, prep.PolicyVersion
	for _, b := range prep.Blockers {
		st.Blockers = append(st.Blockers, Blocker{Code: b.Code, Message: b.Message})
	}
	for _, j := range prep.Jobs {
		check := ""
		if j.Validation != nil {
			check = j.Validation.Check
		}
		st.Jobs = append(st.Jobs, JobRef{ID: j.ID, Check: check, State: j.State})
	}

	switch {
	case len(prep.Conflicts) > 0 || prep.Problem != "":
		st.Mergeable = No
	case prep.CandidateTree == "":
		st.Mergeable = Unknown
	default:
		st.Mergeable = Yes
	}

	failed, pending, missing, passed := false, false, false, 0
	for _, v := range prep.Validations {
		switch v.State {
		case "failed":
			failed = true
		case "passed":
			passed++
		case "pending":
			pending = true
		default:
			missing = true
		}
	}
	switch {
	case failed || prep.HasBlocker(agwp.BlockerFailedValidation):
		st.Validated = No
	case len(prep.Validations) == 0 && !prep.HasBlocker(agwp.BlockerMissingValidation):
		st.Validated = NA
	case missing || prep.HasBlocker(agwp.BlockerMissingValidation) && !pending:
		st.Validated = No
	case pending:
		st.Validated = Pending
	case passed == len(prep.Validations):
		st.Validated = Yes
	default:
		st.Validated = Unknown
	}

	if prep.HasBlocker(agwp.BlockerInsufficientApproval) || prep.HasBlocker(agwp.BlockerChangesRequested) {
		st.Approved = No
	} else {
		st.Approved = Yes
	}

	st.Ready = st.Mergeable == Yes
	for _, b := range prep.Blockers {
		// "actor_not_allowed" is about the orchestrator's own identity, which
		// never integrates; it says nothing about the proposal.
		if b.Code != agwp.BlockerActorNotAllowed {
			st.Ready = false
		}
	}
	return st
}

// forgetClosed drops proposals that are no longer open and remembers the
// integrated ones.
func (s *Scheduler) forgetClosed(ctx context.Context, open map[string]bool) {
	s.mu.Lock()
	var gone []string
	for id := range s.entries {
		if !open[id] {
			gone = append(gone, id)
		}
	}
	s.mu.Unlock()
	if len(gone) == 0 {
		return
	}
	integrated := map[string]agwp.Proposal{}
	if list, err := s.o.API.ListProposals(ctx, s.o.Repo.Name, agwp.ProposalFilter{State: agwp.ProposalIntegrated, Stream: s.o.Repo.Stream}); err == nil {
		for _, p := range list {
			integrated[p.ID] = p
		}
	}
	s.mu.Lock()
	for _, id := range gone {
		e := s.entries[id]
		delete(s.entries, id)
		if p, ok := integrated[id]; ok && e != nil {
			st := e.status
			st.Integrated, st.Ready, st.Head = true, false, p.Head
			st.Mergeable, st.Validated, st.Approved = Yes, orNA(st.Validated), Yes
			s.recent = append([]ProposalStatus{st}, s.recent...)
			if len(s.recent) > s.o.Recent {
				s.recent = s.recent[:s.o.Recent]
			}
		}
	}
	listeners := append([]func(string){}, s.forgetters...)
	s.mu.Unlock()
	for _, id := range gone {
		for _, fn := range listeners {
			fn(id)
		}
	}
}

// SubscribeForget registers a listener called with the id of every proposal
// that is no longer open (integrated or closed). Listeners must not block.
func (s *Scheduler) SubscribeForget(fn func(proposal string)) {
	s.mu.Lock()
	s.forgetters = append(s.forgetters, fn)
	s.mu.Unlock()
}

func orNA(v string) string {
	if v == Yes || v == NA {
		return v
	}
	return Yes // it landed, so whatever the policy demanded was satisfied
}

// Status returns a snapshot for the admin API.
func (s *Scheduler) Status() Status {
	s.mu.Lock()
	defer s.mu.Unlock()
	st := Status{Repo: s.o.Repo.Name, Stream: s.o.Repo.Stream, StreamHead: s.head.Head, StreamVersion: s.head.Version,
		Integrated: append([]ProposalStatus(nil), s.recent...), Jobs: append([]agwp.Job(nil), s.jobs...),
		Queue: append([]agwp.QueueEntry(nil), s.queue...), LastPass: s.lastPass, LastError: s.lastErr}
	for _, e := range s.entries {
		st.Open = append(st.Open, e.status)
	}
	sort.Slice(st.Open, func(i, j int) bool { return st.Open[i].ID < st.Open[j].ID })
	return st
}
