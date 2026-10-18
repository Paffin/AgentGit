package review

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
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
)

// MarkerPrefix starts the machine-readable line of every reviewer comment.
const MarkerPrefix = "ag-review/1 head="

const (
	defaultImportFiles = 60
	defaultImportBytes = 128 << 10
	maxPerPass         = 25
)

// API is what the reviewers call (satisfied by *agwp.Client). The only
// mutation is ReviewProposal with the "comment" decision.
type API interface {
	ListProposals(ctx context.Context, repo string, f agwp.ProposalFilter) ([]agwp.Proposal, error)
	GetProposal(ctx context.Context, repo, id string) (*agwp.ProposalDetail, error)
	ReviewProposal(ctx context.Context, repo, id, decision, body string) (*agwp.Review, error)
	MergeBases(ctx context.Context, repo, a, b string) ([]string, error)
	Diff(ctx context.Context, repo, from, to string, contextLines int) (*agwp.DiffResult, error)
	ReadHead(ctx context.Context, repo, stream string) (*agwp.Stream, error)
	GetTree(ctx context.Context, repo, rev string) (*agwp.Tree, error)
	GetFile(ctx context.Context, repo, rev, path string, maxBytes int64) ([]byte, error)
}

// Options wires the reviewers of one repository.
type Options struct {
	Repo     config.Repo
	API      API
	Reviewer string // identity the comments are posted as (informational)
	Clock    clock.Clock
	Metrics  *metrics.Service
	Log      *slog.Logger
	Loop     config.Loop
}

// Record remembers one review.
type Record struct {
	Proposal string    `json:"proposal"`
	Head     string    `json:"head"`
	At       time.Time `json:"at"`
	Findings []Finding `json:"findings"`
	Summary  Summary   `json:"summary"`
	Posted   bool      `json:"posted"`
	Error    string    `json:"error,omitempty"`
}

// Status is the reviewers' part of /status.
type Status struct {
	Repo     string    `json:"repo"`
	Reviewer string    `json:"reviewer"`
	Recent   []Record  `json:"recent"`
	LastPass time.Time `json:"last_pass,omitempty"`
}

// Component reviews new proposal heads.
type Component struct {
	o        Options
	log      *slog.Logger
	settings Settings
	kick     *loop.Kicker
	run      *loop.Runner

	mu       sync.Mutex
	reviewed map[string]string // proposal -> head already handled
	recent   []Record
	lastPass time.Time
}

// New compiles the repository's review settings.
func New(o Options) (*Component, error) {
	if o.Clock == nil {
		o.Clock = clock.Real{}
	}
	if o.Metrics == nil {
		o.Metrics = metrics.NewService(metrics.NewRegistry())
	}
	if o.Log == nil {
		o.Log = slog.Default()
	}
	rc := o.Repo.Review
	st := Settings{MaxFiles: rc.MaxFiles, MaxLines: rc.MaxLines, Secrets: config.On(rc.Secrets), Tests: config.On(rc.Tests),
		ImportFiles: rc.ImportFiles, ImportBytes: rc.ImportBytes}
	var err error
	if st.Protected, err = glob.CompileAll(rc.Protected); err != nil {
		return nil, err
	}
	if st.Generated, err = glob.CompileAll(rc.Generated); err != nil {
		return nil, err
	}
	if st.Sources, err = glob.CompileAll(rc.Sources); err != nil {
		return nil, err
	}
	for _, p := range rc.AllowRE {
		re, err := regexp.Compile(p)
		if err != nil {
			return nil, err
		}
		st.Allow = append(st.Allow, re)
	}
	if st.MaxFiles < 1 {
		st.MaxFiles = 40
	}
	if st.MaxLines < 1 {
		st.MaxLines = 1500
	}
	if st.ImportFiles < 1 {
		st.ImportFiles = defaultImportFiles
	}
	if st.ImportBytes < 1 {
		st.ImportBytes = defaultImportBytes
	}
	c := &Component{o: o, log: o.Log.With("repo", o.Repo.Name, "component", "review"), settings: st, kick: loop.NewKicker(), reviewed: map[string]string{}}
	c.run = &loop.Runner{Name: "review", Repo: o.Repo.Name, Interval: o.Loop.Reconcile, Debounce: o.Loop.Debounce, Clock: o.Clock, Metrics: o.Metrics, Log: o.Log}
	return c, nil
}

// SetReviewer records the identity the reviewer's credential really authenticates as (known after the preflight
// check), so the comments name their true author.
func (c *Component) SetReviewer(identity string) {
	c.mu.Lock()
	c.o.Reviewer = identity
	c.mu.Unlock()
}

func (c *Component) reviewerIdentity() string {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.o.Reviewer
}

// Topics are the event prefixes that wake the reviewers.
func Topics() []string { return []string{"proposal"} }

// OnEvent wakes the loop for proposal events.
func (c *Component) OnEvent(agwp.Event) { c.kick.Kick() }

// Kick wakes the loop.
func (c *Component) Kick() { c.kick.Kick() }

// Run reconciles until ctx is done.
func (c *Component) Run(ctx context.Context) { c.run.Run(ctx, c.kick, c.Reconcile) }

// Runs is the number of completed passes.
func (c *Component) Runs() uint64 { return c.run.Runs() }

// Reconcile reviews every open proposal head that has not been reviewed yet.
func (c *Component) Reconcile(ctx context.Context) error {
	repo := c.o.Repo.Name
	open, err := c.o.API.ListProposals(ctx, repo, agwp.ProposalFilter{State: agwp.ProposalOpen, Stream: c.o.Repo.Stream})
	if err != nil {
		return fmt.Errorf("list proposals: %w", err)
	}
	c.mu.Lock()
	c.lastPass = c.o.Clock.Now()
	live := map[string]bool{}
	for _, p := range open {
		live[p.ID] = true
	}
	for id := range c.reviewed {
		if !live[id] {
			delete(c.reviewed, id)
		}
	}
	c.mu.Unlock()
	var firstErr error
	done := 0
	for _, p := range open {
		c.mu.Lock()
		seen := c.reviewed[p.ID] == p.Head
		c.mu.Unlock()
		if seen {
			continue
		}
		if done >= maxPerPass {
			c.kick.Kick() // the rest next time
			break
		}
		done++
		if err := c.review(ctx, p); err != nil {
			c.log.Warn("review failed", "proposal", p.ID, "err", err)
			if firstErr == nil {
				firstErr = fmt.Errorf("review %s: %w", p.ID, err)
			}
			continue
		}
		c.mu.Lock()
		c.reviewed[p.ID] = p.Head
		c.mu.Unlock()
	}
	return firstErr
}

// changes returns the per-file diffs of a proposal against its merge base, whether the server truncated the
// diff, and (for an initial import) how many files could not be inspected.
func (c *Component) changes(ctx context.Context, p agwp.Proposal) ([]agwp.FileDiff, bool, int, error) {
	repo := c.o.Repo.Name
	head, err := c.o.API.ReadHead(ctx, repo, c.o.Repo.Stream)
	if err != nil {
		return nil, false, 0, fmt.Errorf("read the stream: %w", err)
	}
	if head.Head == "" {
		files, uninspected, err := c.rootFiles(ctx, p)
		return files, false, uninspected, err
	}
	bases, err := c.o.API.MergeBases(ctx, repo, head.Head, p.Head)
	if err != nil {
		return nil, false, 0, fmt.Errorf("merge base: %w", err)
	}
	from := head.Head
	if len(bases) > 0 {
		from = bases[0]
	}
	d, err := c.o.API.Diff(ctx, repo, from, p.Head, 0)
	if err != nil {
		return nil, false, 0, fmt.Errorf("diff: %w", err)
	}
	return d.Files, d.Truncated, 0, nil
}

// rootFiles treats every file of a proposal into an empty stream as added. Content is read for at most
// ImportFiles files of at most ImportBytes each; the files that were not read (too many, too large, or a download
// that failed) are still listed, with no lines and no patch, and counted in the second result.
func (c *Component) rootFiles(ctx context.Context, p agwp.Proposal) ([]agwp.FileDiff, int, error) {
	repo := c.o.Repo.Name
	tree, err := c.o.API.GetTree(ctx, repo, p.Head)
	if err != nil {
		return nil, 0, fmt.Errorf("tree: %w", err)
	}
	var out []agwp.FileDiff
	uninspected := 0
	for i, e := range tree.Entries {
		fd := agwp.FileDiff{Path: e.Path, Status: "added"}
		read := false
		if i < c.settings.ImportFiles {
			b, err := c.o.API.GetFile(ctx, repo, p.Head, e.Path, int64(c.settings.ImportBytes))
			switch {
			case err != nil:
				c.log.Debug("initial import: a file could not be read", "proposal", p.ID, "path", e.Path, "err", err)
			case isBinary(b):
				fd.Binary, read = true, true
			default:
				lines := strings.Split(strings.TrimSuffix(string(b), "\n"), "\n")
				fd.Additions, fd.Patch, read = len(lines), patchOf(lines), true
			}
		}
		if !read {
			uninspected++
		}
		out = append(out, fd)
	}
	return out, uninspected, nil
}

func isBinary(b []byte) bool {
	for _, c := range b {
		if c == 0 {
			return true
		}
	}
	return false
}

func patchOf(lines []string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "--- /dev/null\n+++ b/f\n@@ -0,0 +1,%d @@\n", len(lines))
	for _, l := range lines {
		b.WriteString("+" + l + "\n")
	}
	return b.String()
}

func (c *Component) review(ctx context.Context, p agwp.Proposal) error {
	repo := c.o.Repo.Name
	files, truncated, uninspected, err := c.changes(ctx, p)
	if err != nil {
		return err
	}
	findings, sum := Analyze(files, c.settings)
	sum.Truncated, sum.Uninspected = truncated, uninspected
	rec := Record{Proposal: p.ID, Head: p.Head, At: c.o.Clock.Now(), Findings: findings, Summary: sum}
	defer func() { c.remember(rec) }()
	if len(findings) == 0 && !truncated && uninspected == 0 {
		return nil // nothing to say; a clean change gets no comment
	}
	digest := digestOf(findings)
	detail, err := c.o.API.GetProposal(ctx, repo, p.ID)
	if err != nil {
		rec.Error = err.Error()
		return fmt.Errorf("read the proposal: %w", err)
	}
	marker := MarkerPrefix + p.Head + " digest=" + digest
	for _, r := range detail.Reviews {
		if r.Decision == agwp.DecisionComment && strings.Contains(r.Body, marker) {
			rec.Posted = true
			return nil
		}
	}
	if detail.Proposal.State != agwp.ProposalOpen || detail.Proposal.Head != p.Head {
		return nil // moved on; the next head gets its own review
	}
	body := Render(p.Head, c.reviewerIdentity(), findings, sum, digest)
	if _, err := c.o.API.ReviewProposal(ctx, repo, p.ID, agwp.DecisionComment, body); err != nil {
		if agwp.IsCode(err, agwp.CodeFailedPrecond, agwp.CodeNotFound) {
			return nil
		}
		rec.Error = err.Error()
		return fmt.Errorf("post the comment: %w", err)
	}
	rec.Posted = true
	for _, f := range findings {
		c.o.Metrics.ReviewsPosted.With(repo, "review", f.Kind).Inc()
	}
	c.log.Info("posted automated review", "proposal", p.ID, "head", short(p.Head), "findings", len(findings))
	return nil
}

func digestOf(fs []Finding) string {
	h := sha256.New()
	for _, f := range fs {
		fmt.Fprintf(h, "%s|%s|%s\n", f.Kind, f.Severity, strings.Join(f.Details, "\x1f"))
	}
	return hex.EncodeToString(h.Sum(nil))[:12]
}

func (c *Component) remember(r Record) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.recent = append([]Record{r}, c.recent...)
	if len(c.recent) > 30 {
		c.recent = c.recent[:30]
	}
}

// Status returns the recent reviews.
func (c *Component) Status() Status {
	c.mu.Lock()
	defer c.mu.Unlock()
	recent := append([]Record{}, c.recent...)
	sort.SliceStable(recent, func(i, j int) bool { return recent[i].At.After(recent[j].At) })
	return Status{Repo: c.o.Repo.Name, Reviewer: c.o.Reviewer, Recent: recent, LastPass: c.lastPass}
}

// ---- rendering ---------------------------------------------------------------------

// Render writes the comment body.
func Render(head, reviewer string, findings []Finding, sum Summary, digest string) string {
	var b strings.Builder
	b.WriteString("**Automated review** — deterministic checks, advisory\n\n")
	fmt.Fprintf(&b, "Head `%s` · %d file(s) · +%d −%d", short(head), sum.Files, sum.Added, sum.Removed)
	if sum.Truncated {
		b.WriteString(" · _the diff was truncated by the server, so only part of the change was inspected_")
	}
	if sum.Uninspected > 0 {
		fmt.Fprintf(&b, " · _%d file(s) of this initial import were not inspected (too many files, too large, or unreadable), so the checks above did not see them_", sum.Uninspected)
	}
	b.WriteString("\n\n")
	for _, f := range findings {
		fmt.Fprintf(&b, "- **%s** (%s)\n", f.Title, f.Severity)
		for _, d := range f.Details {
			fmt.Fprintf(&b, "  - %s\n", d)
		}
	}
	if len(findings) == 0 {
		b.WriteString("No findings.\n")
	}
	b.WriteString("\n<sub>These are heuristics run by a service identity")
	if reviewer != "" {
		fmt.Fprintf(&b, " (%s)", reviewer)
	}
	b.WriteString(". This is a `comment` review: it is neither an approval nor a request for changes, and whether an automated reviewer's approval could ever count is a decision of the stream policy, not of this service.</sub>\n")
	fmt.Fprintf(&b, "\n<!-- %s%s digest=%s -->\n", MarkerPrefix, head, digest)
	return b.String()
}

func short(s string) string {
	if len(s) > 12 {
		return s[:12]
	}
	return s
}
