package semindex

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"sort"
	"sync"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/glob"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/loop"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
)

// API is the read side of AGWP the index needs (satisfied by *agwp.Client).
type API interface {
	ReadHead(ctx context.Context, repo, stream string) (*agwp.Stream, error)
	GetTree(ctx context.Context, repo, rev string) (*agwp.Tree, error)
	GetFile(ctx context.Context, repo, rev, path string, maxBytes int64) ([]byte, error)
	ListProposals(ctx context.Context, repo string, f agwp.ProposalFilter) ([]agwp.Proposal, error)
	MergeBases(ctx context.Context, repo, a, b string) ([]string, error)
	Diff(ctx context.Context, repo, from, to string, contextLines int) (*agwp.DiffResult, error)
}

// Options wires the index of one repository.
type Options struct {
	Repo    config.Repo
	API     API
	Store   *Store // nil keeps the index in memory only
	Clock   clock.Clock
	Metrics *metrics.Service
	Log     *slog.Logger
	Loop    config.Loop
}

// Component keeps one repository's index current.
type Component struct {
	o    Options
	log  *slog.Logger
	skip glob.Set
	idx  *Index
	kick *loop.Kicker
	run  *loop.Runner

	mu       sync.Mutex
	analyses map[string]*ProposalSymbols // by proposal id
}

// New builds the component and loads whatever was stored earlier.
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
	skip, err := glob.CompileAll(o.Repo.Index.Skip)
	if err != nil {
		return nil, err
	}
	c := &Component{o: o, log: o.Log.With("repo", o.Repo.Name, "component", "semindex"), skip: skip, kick: loop.NewKicker(), analyses: map[string]*ProposalSymbols{}}
	c.idx = NewIndex(o.Repo.Name)
	if o.Store != nil {
		x, bad, err := o.Store.Load(o.Repo.Name, o.Repo.Index.MaxFiles)
		if err != nil {
			return nil, err
		}
		c.idx = x
		if bad > 0 {
			c.log.Warn("some stored index entries were unreadable and will be rebuilt", "entries", bad)
		}
		st := x.Stats()
		o.Metrics.IndexFiles.With(o.Repo.Name).Set(float64(st.Files))
		o.Metrics.IndexSymbols.With(o.Repo.Name).Set(float64(st.Symbols))
	}
	c.run = &loop.Runner{Name: "semindex", Repo: o.Repo.Name, Interval: o.Loop.Reconcile, Debounce: o.Loop.Debounce, Clock: o.Clock, Metrics: o.Metrics, Log: o.Log}
	return c, nil
}

// Topics are the events that wake the index.
func Topics() []string { return []string{"stream.integrate", "stream.cas", "proposal"} }

// OnEvent wakes the loop.
func (c *Component) OnEvent(agwp.Event) { c.kick.Kick() }

// Kick wakes the loop.
func (c *Component) Kick() { c.kick.Kick() }

// Run reconciles until ctx is done.
func (c *Component) Run(ctx context.Context) { c.run.Run(ctx, c.kick, c.Reconcile) }

// Runs is the number of completed passes.
func (c *Component) Runs() uint64 { return c.run.Runs() }

// Index exposes the in-memory index.
func (c *Component) Index() *Index { return c.idx }

// Repo is the repository name.
func (c *Component) Repo() string { return c.o.Repo.Name }

// Reconcile brings the index up to the stream head and refreshes the
// per-proposal symbol changes.
func (c *Component) Reconcile(ctx context.Context) error {
	serr := c.Sync(ctx)
	oerr := c.refreshProposals(ctx)
	if serr != nil {
		return serr
	}
	return oerr
}

// Sync updates the index incrementally to the stream's current head: only
// files whose blob changed are fetched.
func (c *Component) Sync(ctx context.Context) error {
	repo, stream := c.o.Repo.Name, c.o.Repo.Stream
	head, err := c.o.API.ReadHead(ctx, repo, stream)
	if err != nil {
		return fmt.Errorf("read the stream head: %w", err)
	}
	if head.Head == "" || head.Head == c.idx.Head() {
		return nil
	}
	tree, err := c.o.API.GetTree(ctx, repo, head.Head)
	if err != nil {
		return fmt.Errorf("read the tree: %w", err)
	}
	cfg := c.o.Repo.Index
	var sk Skipped
	wanted := map[string]agwp.TreeEntry{}
	entries := append([]agwp.TreeEntry(nil), tree.Entries...)
	sort.Slice(entries, func(i, j int) bool { return entries[i].Path < entries[j].Path })
	for _, e := range entries {
		switch {
		case e.FileType != "" && e.FileType != "regular" && e.FileType != "file": // "regular" today; "file" on older servers
			continue
		case c.skip.Match(e.Path):
			sk.Pattern++
		case LanguageOf(e.Path) == "":
			sk.Unsupported++
		default:
			wanted[e.Path] = e
		}
	}
	known := c.idx.Known()
	var put []*FileEntry
	var remove []string
	var firstErr error
	paths := make([]string, 0, len(wanted))
	for p := range wanted {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	// The file limit counts files that end up indexed; files that turn out to be
	// binary or too large do not use it up. Fetch attempts are bounded as well.
	indexed, attempts := 0, 0
	maxAttempts := 4*cfg.MaxFiles + 100
	kept := map[string]bool{}
	for _, p := range paths {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if indexed >= cfg.MaxFiles || attempts >= maxAttempts {
			sk.OverLimit++
			continue
		}
		e := wanted[p]
		if old, ok := c.idx.Entry(p); ok && old.Blob == e.BlobID {
			indexed++
			kept[p] = true
			continue
		}
		attempts++
		b, err := c.o.API.GetFile(ctx, repo, head.Head, p, int64(cfg.MaxBytes))
		switch {
		case errors.Is(err, agwp.ErrResponseTooLarge):
			sk.TooLarge++
			continue
		case err != nil:
			sk.Errors++
			if firstErr == nil {
				firstErr = fmt.Errorf("fetch %s: %w", p, err)
			}
			if _, ok := c.idx.Entry(p); ok {
				kept[p] = true // keep the stale entry rather than lose the file
				indexed++
			}
			continue
		case bytes.IndexByte(b, 0) >= 0:
			sk.Binary++
			continue
		}
		put = append(put, &FileEntry{Path: p, Blob: e.BlobID, Syms: Extract(LanguageOf(p), b, known)})
		kept[p] = true
		indexed++
	}
	for _, p := range c.idx.Paths() {
		if !kept[p] {
			remove = append(remove, p)
		}
	}
	newHead := head.Head
	if firstErr != nil {
		newHead = c.idx.Head() // not complete: look again next time (unchanged blobs are skipped cheaply)
	}
	c.idx.apply(put, remove, newHead, sk, firstErr)
	if st := c.o.Store; st != nil {
		for _, e := range put {
			if err := st.SaveEntry(e); err != nil {
				c.log.Warn("could not persist an index entry", "path", e.Path, "err", err)
			}
		}
		for _, p := range remove {
			_ = st.DeleteEntry(p)
		}
		if firstErr == nil {
			if err := st.SaveHead(repo, newHead); err != nil {
				c.log.Warn("could not persist the index head", "err", err)
			}
		}
	}
	stats := c.idx.Stats()
	c.o.Metrics.IndexFiles.With(repo).Set(float64(stats.Files))
	c.o.Metrics.IndexSymbols.With(repo).Set(float64(stats.Symbols))
	c.o.Metrics.IndexUpdates.With(repo).Inc()
	c.log.Info("index updated", "head", short(head.Head), "files", stats.Files, "symbols", stats.Symbols, "changed", len(put), "removed", len(remove))
	return firstErr
}

func short(s string) string {
	if len(s) > 12 {
		return s[:12]
	}
	return s
}

// Status is the index's part of /status.
type Status struct {
	Stats
	Proposals int `json:"proposals_analysed"`
}

// Status returns a snapshot.
func (c *Component) Status() Status {
	c.mu.Lock()
	n := len(c.analyses)
	c.mu.Unlock()
	return Status{Stats: c.idx.Stats(), Proposals: n}
}
