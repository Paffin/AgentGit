package advisor

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"log/slog"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/execsched"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/loop"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

// API is what the component calls (satisfied by *agwp.Client). Posting uses a
// credential that can only comment: ReviewProposal is called with the
// "comment" decision and nothing else, ever.
type API interface {
	GetFile(ctx context.Context, repo, rev, path string, maxBytes int64) ([]byte, error)
	GetProposal(ctx context.Context, repo, id string) (*agwp.ProposalDetail, error)
	ReviewProposal(ctx context.Context, repo, id, decision, body string) (*agwp.Review, error)
}

// Options wires the component.
type Options struct {
	Repo    config.Repo
	API     API
	Advisor Advisor
	Clock   clock.Clock
	Metrics *metrics.Service
	Log     *slog.Logger
	// Remember bounds the stored records (default 50).
	Remember int
}

// Record is what the advisor remembers about one proposal.
type Record struct {
	Proposal    string       `json:"proposal"`
	Head        string       `json:"head"`
	StreamHead  string       `json:"stream_head"`
	Base        string       `json:"base"`
	At          time.Time    `json:"at"`
	Digest      string       `json:"digest"`
	Posted      bool         `json:"posted"`
	ReviewID    int64        `json:"review_id,omitempty"`
	Error       string       `json:"error,omitempty"`
	Suggestions []Suggestion `json:"suggestions"`
}

// Summary is a Record without file contents, for /status.
type Summary struct {
	Proposal   string    `json:"proposal"`
	Head       string    `json:"head"`
	StreamHead string    `json:"stream_head"`
	At         time.Time `json:"at"`
	Posted     bool      `json:"posted"`
	Error      string    `json:"error,omitempty"`
	Files      []string  `json:"files"`
	Classes    []string  `json:"classes"`
	Complete   int       `json:"complete_suggestions"`
}

// Status is the advisor's part of /status.
type Status struct {
	Repo    string    `json:"repo"`
	Advisor string    `json:"advisor"`
	Records []Summary `json:"records"`
}

// Component posts conflict suggestions for one repository.
type Component struct {
	o    Options
	log  *slog.Logger
	kick *loop.Kicker

	mu      sync.Mutex
	pending map[string]execsched.Prepared
	records map[string]*Record
	order   []string
	done    map[string]string // proposal -> processed key
}

// New builds the component.
func New(o Options) *Component {
	if o.Clock == nil {
		o.Clock = clock.Real{}
	}
	if o.Metrics == nil {
		o.Metrics = metrics.NewService(metrics.NewRegistry())
	}
	if o.Log == nil {
		o.Log = slog.Default()
	}
	if o.Remember <= 0 {
		o.Remember = 50
	}
	if o.Advisor == nil {
		o.Advisor = Rules{MaxFileBytes: o.Repo.Advisor.MaxFile}
	}
	return &Component{o: o, log: o.Log.With("repo", o.Repo.Name, "component", "advisor"), kick: loop.NewKicker(),
		pending: map[string]execsched.Prepared{}, records: map[string]*Record{}, done: map[string]string{}}
}

// OnPrepared receives every preparation from the execution scheduler. It only
// queues work; the analysis runs on the component's own goroutine.
func (c *Component) OnPrepared(p execsched.Prepared) {
	c.mu.Lock()
	if len(p.Prep.Conflicts) == 0 {
		// resolved or never conflicted: forget the old advice
		delete(c.records, p.Proposal.ID)
		delete(c.done, p.Proposal.ID)
		delete(c.pending, p.Proposal.ID)
		c.mu.Unlock()
		return
	}
	c.pending[p.Proposal.ID] = p
	c.mu.Unlock()
	c.kick.Kick()
}

// Run processes queued preparations until ctx is done.
func (c *Component) Run(ctx context.Context) {
	for {
		c.drain(ctx)
		select {
		case <-ctx.Done():
			return
		case <-c.kick.C():
		}
	}
}

func (c *Component) drain(ctx context.Context) {
	c.mu.Lock()
	work := make([]execsched.Prepared, 0, len(c.pending))
	for _, p := range c.pending {
		work = append(work, p)
	}
	c.pending = map[string]execsched.Prepared{}
	c.mu.Unlock()
	sort.Slice(work, func(i, j int) bool { return work[i].Proposal.ID < work[j].Proposal.ID })
	for _, p := range work {
		if ctx.Err() != nil {
			return
		}
		if err := c.Process(ctx, p); err != nil && ctx.Err() == nil {
			c.log.Warn("advice could not be completed", "proposal", p.Proposal.ID, "err", err)
		}
	}
}

func conflictKey(p execsched.Prepared) string {
	paths := make([]string, 0, len(p.Prep.Conflicts))
	for _, cf := range p.Prep.Conflicts {
		paths = append(paths, cf.Path+"|"+cf.Kind)
	}
	sort.Strings(paths)
	return strings.Join([]string{p.Proposal.Head, p.StreamHead, p.Prep.Base, strings.Join(paths, ";")}, "@")
}

// Process analyses one preparation and posts the suggestions if the same advice
// was not posted before. Safe to call from tests.
func (c *Component) Process(ctx context.Context, p execsched.Prepared) error {
	if len(p.Prep.Conflicts) == 0 {
		return nil
	}
	key := conflictKey(p)
	c.mu.Lock()
	if c.done[p.Proposal.ID] == key {
		c.mu.Unlock()
		return nil
	}
	c.mu.Unlock()

	cf := append([]agwp.Conflict(nil), p.Prep.Conflicts...)
	sort.Slice(cf, func(i, j int) bool { return cf[i].Path < cf[j].Path })
	omitted := 0
	if max := c.o.Repo.Advisor.MaxPaths; max > 0 && len(cf) > max {
		omitted, cf = len(cf)-max, cf[:max]
	}
	sugs := make([]Suggestion, 0, len(cf))
	for _, conf := range cf {
		in, err := c.load(ctx, p, conf)
		if err != nil {
			return fmt.Errorf("fetch versions of %s: %w", conf.Path, err)
		}
		s, err := c.o.Advisor.Suggest(ctx, in)
		if err != nil {
			return fmt.Errorf("analyse %s: %w", conf.Path, err)
		}
		c.o.Metrics.AdvisorSuggest.With(c.o.Repo.Name, s.Class).Inc()
		sugs = append(sugs, s)
	}
	digest := digestOf(sugs)
	rec := &Record{Proposal: p.Proposal.ID, Head: p.Proposal.Head, StreamHead: p.StreamHead, Base: p.Prep.Base, At: c.o.Clock.Now(), Digest: digest, Suggestions: sugs}
	if omitted > 0 {
		rec.Suggestions = append(rec.Suggestions, Suggestion{Path: fmt.Sprintf("(%d more conflicting file(s))", omitted), Advisor: c.o.Advisor.Name(), Class: ClassNeedsPerson,
			Notes: []string{"not analysed: the per-proposal limit of files was reached"}})
	}

	posted, reviewID, err := c.post(ctx, p, rec)
	rec.Posted, rec.ReviewID = posted, reviewID
	if err != nil {
		rec.Error = err.Error()
	}
	c.remember(rec)
	if err == nil {
		c.mu.Lock()
		c.done[p.Proposal.ID] = key
		c.mu.Unlock()
	}
	return err
}

// load fetches the three versions of a conflicting path.
func (c *Component) load(ctx context.Context, p execsched.Prepared, conf agwp.Conflict) (Input, error) {
	in := Input{Path: conf.Path, Kind: conf.Kind}
	max := int64(c.o.Repo.Advisor.MaxFile)
	if max <= 0 {
		max = 1 << 20
	}
	get := func(rev string) (*File, error) {
		if rev == "" {
			return nil, nil
		}
		b, err := c.o.API.GetFile(ctx, c.o.Repo.Name, rev, conf.Path, max)
		switch {
		case err == nil:
			return &File{Content: b}, nil
		case agwp.IsNotFound(err):
			return nil, nil
		case errors.Is(err, agwp.ErrResponseTooLarge):
			return &File{Content: make([]byte, max+1)}, nil // reported as "too large" by the rules
		}
		return nil, err
	}
	var err error
	if in.Base, err = get(p.Prep.Base); err != nil {
		return in, err
	}
	if in.Ours, err = get(p.StreamHead); err != nil {
		return in, err
	}
	if in.Theirs, err = get(p.Proposal.Head); err != nil {
		return in, err
	}
	return in, nil
}

// post writes the comment unless an equal one is already on the proposal.
func (c *Component) post(ctx context.Context, p execsched.Prepared, rec *Record) (bool, int64, error) {
	repo := c.o.Repo.Name
	detail, err := c.o.API.GetProposal(ctx, repo, p.Proposal.ID)
	if err != nil {
		return false, 0, fmt.Errorf("read the proposal: %w", err)
	}
	marker := MarkerPrefix + rec.Digest
	for _, r := range detail.Reviews {
		if r.Head == detail.Proposal.Head && r.Decision == agwp.DecisionComment && strings.Contains(r.Body, marker) {
			return true, r.ID, nil // already said
		}
	}
	if detail.Proposal.State != agwp.ProposalOpen {
		return false, 0, nil
	}
	body := Render(c.o.Advisor.Name(), detail.Proposal.Head, p.StreamHead, p.Proposal.Stream, rec.Suggestions, c.o.Repo.Advisor.Inline, rec.Digest)
	rv, err := c.o.API.ReviewProposal(ctx, repo, p.Proposal.ID, agwp.DecisionComment, body)
	if err != nil {
		if agwp.IsCode(err, agwp.CodeFailedPrecond, agwp.CodeNotFound) {
			return false, 0, nil // closed or integrated in the meantime
		}
		return false, 0, fmt.Errorf("post the comment: %w", err)
	}
	c.o.Metrics.ReviewsPosted.With(repo, "advisor", "conflict").Inc()
	c.log.Info("posted conflict advice", "proposal", p.Proposal.ID, "files", len(rec.Suggestions), "review", rv.ID)
	return true, rv.ID, nil
}

func digestOf(sugs []Suggestion) string {
	h := sha256.New()
	for _, s := range sugs {
		fmt.Fprintf(h, "%s|%s|%s|%s|%v|", s.Path, s.Advisor, s.Class, s.SHA256, s.Complete)
		for _, r := range s.Regions {
			fmt.Fprintf(h, "%d-%d:%s:%s;", r.BaseStart, r.BaseEnd, r.Class, r.Resolution)
		}
		h.Write([]byte{'\n'})
	}
	return hex.EncodeToString(h.Sum(nil))[:16]
}

func (c *Component) remember(rec *Record) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if _, ok := c.records[rec.Proposal]; !ok {
		c.order = append(c.order, rec.Proposal)
	}
	c.records[rec.Proposal] = rec
	for len(c.order) > c.o.Remember {
		delete(c.records, c.order[0])
		c.order = c.order[1:]
	}
}

// Forget drops everything remembered about a proposal that is no longer open
// (integrated or closed): its advice is moot and must not pile up.
func (c *Component) Forget(proposal string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	delete(c.records, proposal)
	delete(c.done, proposal)
	delete(c.pending, proposal)
	for i, id := range c.order {
		if id == proposal {
			c.order = append(c.order[:i], c.order[i+1:]...)
			break
		}
	}
}

// Suggestions returns the full stored advice for a proposal (with the
// suggested files), for the status API.
func (c *Component) Suggestions(proposal string) (Record, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	r, ok := c.records[proposal]
	if !ok {
		return Record{}, false
	}
	return *r, true
}

// Status summarises what the advisor knows.
func (c *Component) Status() Status {
	c.mu.Lock()
	defer c.mu.Unlock()
	st := Status{Repo: c.o.Repo.Name, Advisor: c.o.Advisor.Name(), Records: []Summary{}}
	for _, id := range c.order {
		r := c.records[id]
		if r == nil {
			continue
		}
		s := Summary{Proposal: r.Proposal, Head: r.Head, StreamHead: r.StreamHead, At: r.At, Posted: r.Posted, Error: r.Error}
		classes := map[string]bool{}
		for _, sg := range r.Suggestions {
			s.Files = append(s.Files, sg.Path)
			classes[sg.Class] = true
			if sg.Complete {
				s.Complete++
			}
		}
		for k := range classes {
			s.Classes = append(s.Classes, k)
		}
		sort.Strings(s.Classes)
		st.Records = append(st.Records, s)
	}
	return st
}
