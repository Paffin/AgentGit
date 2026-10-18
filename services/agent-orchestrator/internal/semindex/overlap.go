package semindex

import (
	"context"
	"fmt"
	"sort"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/textdiff"
)

// How a definition changed in a proposal.
const (
	ChangeAdded     = "added"
	ChangeRemoved   = "removed"
	ChangeSignature = "signature"
	ChangeModified  = "modified"
)

// Changed is one definition a proposal changes.
type Changed struct {
	Path      string `json:"path"`
	Name      string `json:"name"`
	Container string `json:"container,omitempty"`
	Kind      string `json:"kind"`
	Change    string `json:"change"`
	Exported  bool   `json:"exported"`
}

// Qualified is Container.Name or Name.
func (c Changed) Qualified() string {
	if c.Container == "" {
		return c.Name
	}
	return c.Container + "." + c.Name
}

// ProposalSymbols is the set of definitions one proposal changes.
type ProposalSymbols struct {
	Proposal string    `json:"proposal"`
	Title    string    `json:"title"`
	Head     string    `json:"head"`
	Base     string    `json:"base"`
	Files    []string  `json:"files"`
	Changed  []Changed `json:"changed"`
	// Partial is set when files were skipped (too many, too large, unsupported).
	Partial bool      `json:"partial,omitempty"`
	At      time.Time `json:"at"`
	// Mentions are the identifiers that occur in the changed files at the proposal's head.
	Mentions map[string]bool `json:"-"`
	// Description and Patches are kept for a Judge only (see judge.go): the proposal's text and the diffs of its
	// changed files, bounded. Neither is served.
	Description string            `json:"-"`
	Patches     map[string]string `json:"-"`
}

// Overlap kinds.
const (
	KindDefinition = "definition_overlap"
	KindName       = "name_overlap"
	KindCaller     = "caller_affected"
)

// SymbolRef names a symbol involved in an overlap.
type SymbolRef struct {
	Name    string `json:"name"`
	Kind    string `json:"kind"`
	PathA   string `json:"path_a,omitempty"`
	PathB   string `json:"path_b,omitempty"`
	ChangeA string `json:"change_a,omitempty"`
	ChangeB string `json:"change_b,omitempty"`
}

// Overlap says two open proposals touch the same symbols — richer than "they
// touch the same paths", and advisory like everything here.
type Overlap struct {
	A        string      `json:"a"`
	B        string      `json:"b"`
	Kind     string      `json:"kind"`
	Severity string      `json:"severity"` // high | medium | low
	Symbols  []SymbolRef `json:"symbols"`
	Why      string      `json:"why"`
	// Assessment is a language model's opinion on whether the overlap matters (model mode only). Severity above is
	// the static analysis's own and is not changed by it.
	Assessment *Assessment `json:"assessment,omitempty"`
}

// analyze computes the definitions a proposal changes relative to base.
func (c *Component) analyze(ctx context.Context, p agwp.Proposal, base string) (*ProposalSymbols, error) {
	repo := c.o.Repo.Name
	ctxLines := 0
	if c.o.Judge != nil {
		ctxLines = 3 // a model reads a change better with the lines around it
	}
	diff, err := c.o.API.Diff(ctx, repo, base, p.Head, ctxLines)
	if err != nil {
		return nil, fmt.Errorf("diff: %w", err)
	}
	ps := &ProposalSymbols{Proposal: p.ID, Title: p.Title, Head: p.Head, Base: base, At: c.o.Clock.Now(), Partial: diff.Truncated, Mentions: map[string]bool{}}
	if c.o.Judge != nil {
		ps.Description, ps.Patches = p.Description, map[string]string{}
	}
	skipped := map[string]bool{}
	maxBytes := int64(c.o.Repo.Index.MaxBytes)
	maxFiles := c.o.Repo.Index.ProposalFiles
	for _, f := range diff.Files {
		ps.Files = append(ps.Files, f.Path)
		lang := LanguageOf(f.Path)
		if lang == "" || f.Binary || c.skip.Match(f.Path) {
			continue
		}
		if len(skipped) >= maxFiles {
			ps.Partial = true
			continue
		}
		var oldB, newB []byte
		if f.Status != "added" {
			if oldB, err = c.o.API.GetFile(ctx, repo, base, f.Path, maxBytes); err != nil {
				ps.Partial, skipped[f.Path] = true, true
				continue
			}
		}
		if f.Status != "deleted" {
			if newB, err = c.o.API.GetFile(ctx, repo, p.Head, f.Path, maxBytes); err != nil {
				ps.Partial, skipped[f.Path] = true, true
				continue
			}
		}
		skipped[f.Path] = false
		if ps.Patches != nil && f.Patch != "" {
			ps.Patches[f.Path] = f.Patch
		}
		oldS, newS := Extract(lang, oldB, nil), Extract(lang, newB, nil)
		ps.Changed = append(ps.Changed, compareDefs(f.Path, oldB, newB, oldS.Defs, newS.Defs)...)
		for id := range newS.Idents {
			ps.Mentions[id] = true
		}
	}
	sort.Strings(ps.Files)
	sort.Slice(ps.Changed, func(i, j int) bool {
		a, b := ps.Changed[i], ps.Changed[j]
		if a.Path != b.Path {
			return a.Path < b.Path
		}
		if a.Qualified() != b.Qualified() {
			return a.Qualified() < b.Qualified()
		}
		return a.Kind < b.Kind
	})
	return ps, nil
}

// compareDefs lists the definitions of one file that were added, removed or
// changed. A definition counts as modified when a changed line falls inside
// its span.
func compareDefs(path string, oldB, newB []byte, oldDefs, newDefs []Def) []Changed {
	key := func(d Def) string { return d.Kind + "|" + d.Qualified() }
	newByKey := map[string][]int{}
	for i, d := range newDefs {
		newByKey[key(d)] = append(newByKey[key(d)], i)
	}
	hunks := textdiff.Diff(textdiff.SplitLines(string(oldB)), textdiff.SplitLines(string(newB)))
	touched := func(start, end int, old bool) bool { // 1-based inclusive span
		s, e := start-1, end
		for _, h := range hunks {
			a, b := h.BStart, h.BEnd
			if old {
				a, b = h.AStart, h.AEnd
			}
			if a < e && b > s || a == b && a > s && a < e {
				return true
			}
		}
		return false
	}
	var out []Changed
	used := map[int]bool{}
	for _, d := range oldDefs {
		idxs := newByKey[key(d)]
		if len(idxs) == 0 {
			out = append(out, Changed{Path: path, Name: d.Name, Container: d.Container, Kind: d.Kind, Change: ChangeRemoved, Exported: d.Exported})
			continue
		}
		nd := newDefs[idxs[0]]
		newByKey[key(d)] = idxs[1:]
		used[idxs[0]] = true
		switch {
		case nd.Sig != d.Sig:
			out = append(out, Changed{Path: path, Name: d.Name, Container: d.Container, Kind: d.Kind, Change: ChangeSignature, Exported: nd.Exported || d.Exported})
		case touched(d.Line, d.EndLine, true) || touched(nd.Line, nd.EndLine, false):
			out = append(out, Changed{Path: path, Name: d.Name, Container: d.Container, Kind: d.Kind, Change: ChangeModified, Exported: nd.Exported})
		}
	}
	for i, d := range newDefs {
		if !used[i] {
			out = append(out, Changed{Path: path, Name: d.Name, Container: d.Container, Kind: d.Kind, Change: ChangeAdded, Exported: d.Exported})
		}
	}
	return out
}

// refreshProposals analyses open proposals whose head changed and forgets
// closed ones.
func (c *Component) refreshProposals(ctx context.Context) error {
	repo, stream := c.o.Repo.Name, c.o.Repo.Stream
	open, err := c.o.API.ListProposals(ctx, repo, agwp.ProposalFilter{State: agwp.ProposalOpen, Stream: stream})
	if err != nil {
		return fmt.Errorf("list proposals: %w", err)
	}
	live := map[string]bool{}
	for _, p := range open {
		live[p.ID] = true
	}
	c.mu.Lock()
	for id := range c.analyses {
		if !live[id] {
			delete(c.analyses, id)
		}
	}
	c.mu.Unlock()
	head, err := c.o.API.ReadHead(ctx, repo, stream)
	if err != nil {
		return fmt.Errorf("read the stream head: %w", err)
	}
	var firstErr error
	done := 0
	for _, p := range open {
		c.mu.Lock()
		cur := c.analyses[p.ID]
		c.mu.Unlock()
		if cur != nil && cur.Head == p.Head {
			continue
		}
		if done >= c.o.Repo.Index.ProposalsPerPass {
			c.kick.Kick()
			break
		}
		done++
		base := ""
		if head.Head != "" {
			bases, err := c.o.API.MergeBases(ctx, repo, head.Head, p.Head)
			if err != nil {
				if firstErr == nil {
					firstErr = fmt.Errorf("merge base of %s: %w", p.ID, err)
				}
				continue
			}
			if len(bases) > 0 {
				base = bases[0]
			} else {
				base = head.Head
			}
		}
		if base == "" {
			continue // a proposal into an empty stream: nothing to compare with
		}
		ps, err := c.analyze(ctx, p, base)
		if err != nil {
			if firstErr == nil {
				firstErr = fmt.Errorf("analyse %s: %w", p.ID, err)
			}
			continue
		}
		c.mu.Lock()
		c.analyses[p.ID] = ps
		c.mu.Unlock()
	}
	return firstErr
}

// Changes returns the definitions each analysed open proposal changes.
func (c *Component) Changes() []*ProposalSymbols {
	c.mu.Lock()
	defer c.mu.Unlock()
	out := make([]*ProposalSymbols, 0, len(c.analyses))
	for _, ps := range c.analyses {
		out = append(out, ps)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Proposal < out[j].Proposal })
	return out
}

// Overlaps compares the analysed proposals pairwise. In model mode each overlap carries the model's assessment of
// its pair, when there is one.
func (c *Component) Overlaps() []Overlap {
	ps := c.Changes()
	out := ComputeOverlaps(ps)
	if c.o.Judge == nil {
		return out
	}
	heads := map[string]string{}
	for _, p := range ps {
		heads[p.Proposal] = p.Head
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := range out {
		if a := c.assessments[pairKey(out[i].A, heads[out[i].A], out[i].B, heads[out[i].B])]; a != nil {
			cp := *a
			out[i].Assessment = &cp
		}
	}
	return out
}

// pairKey names a pair of proposals at their current heads, whichever order they are given in.
func pairKey(a, headA, b, headB string) string {
	if a > b {
		a, headA, b, headB = b, headB, a, headA
	}
	return a + "@" + headA + "|" + b + "@" + headB
}

// judgeOverlaps has the model assess the pairs of proposals that the static analysis found overlapping and that it
// has not assessed at their current heads: at most JudgePerPass pairs per pass, the most severe first. Weak hints
// (same-named exports in different files) are not worth a request. A failure is recorded and the pair waits before
// it is tried again; the static findings are unaffected.
func (c *Component) judgeOverlaps(ctx context.Context) {
	ps := c.Changes()
	byID := map[string]*ProposalSymbols{}
	for _, p := range ps {
		byID[p.Proposal] = p
	}
	type pair struct {
		a, b     string
		key      string
		overlaps []Overlap
		rank     int
	}
	rank := map[string]int{"high": 0, "medium": 1, "low": 2}
	pairs := map[string]*pair{}
	for _, o := range ComputeOverlaps(ps) {
		if o.Kind == KindName {
			continue
		}
		a, b := o.A, o.B
		if a > b {
			a, b = b, a
		}
		key := pairKey(a, byID[a].Head, b, byID[b].Head)
		pr := pairs[key]
		if pr == nil {
			pr = &pair{a: a, b: b, key: key, rank: rank[o.Severity]}
			pairs[key] = pr
		}
		pr.rank = min(pr.rank, rank[o.Severity])
		pr.overlaps = append(pr.overlaps, o)
	}
	// forget what no longer applies: assessments and failures of pairs that are gone or at other heads
	c.mu.Lock()
	for k := range c.assessments {
		if pairs[k] == nil {
			delete(c.assessments, k)
		}
	}
	for k := range c.judgeFailed {
		if pairs[k] == nil {
			delete(c.judgeFailed, k)
		}
	}
	todo := make([]*pair, 0, len(pairs))
	now := c.o.Clock.Now()
	for k, pr := range pairs {
		if c.assessments[k] != nil {
			continue
		}
		if at, ok := c.judgeFailed[k]; ok && now.Before(at) {
			continue
		}
		todo = append(todo, pr)
	}
	c.mu.Unlock()
	sort.Slice(todo, func(i, j int) bool {
		if todo[i].rank != todo[j].rank {
			return todo[i].rank < todo[j].rank
		}
		return todo[i].key < todo[j].key
	})
	for i, pr := range todo {
		if ctx.Err() != nil {
			return
		}
		if i >= c.o.JudgePerPass {
			c.kick.Kick()
			return
		}
		view := func(p *ProposalSymbols) ProposalView {
			// only the files that carry the overlap are shown to the model
			relevant := map[string]bool{}
			for _, o := range pr.overlaps {
				for _, s := range o.Symbols {
					relevant[s.PathA], relevant[s.PathB] = true, true
				}
			}
			patches := map[string]string{}
			for path, patch := range p.Patches {
				if relevant[path] {
					patches[path] = patch
				}
			}
			return ProposalView{ID: p.Proposal, Title: p.Title, Description: p.Description, Changed: p.Changed, Patches: patches}
		}
		a, err := c.o.Judge.Judge(ctx, Pair{A: view(byID[pr.a]), B: view(byID[pr.b]), Overlaps: pr.overlaps})
		c.mu.Lock()
		if err != nil {
			c.judgeFailed[pr.key] = c.o.Clock.Now().Add(c.o.JudgeRetry)
			c.judgeErr = err.Error()
			c.mu.Unlock()
			c.log.Warn("the model could not assess an overlap", "a", pr.a, "b", pr.b, "err", err)
			continue
		}
		a.At = c.o.Clock.Now()
		c.assessments[pr.key] = a
		c.judgeErr = ""
		c.mu.Unlock()
		c.log.Info("assessed an overlap", "a", pr.a, "b", pr.b, "verdict", a.Verdict, "severity", a.Severity, "model", a.Model)
	}
}

// ComputeOverlaps finds, for every pair of proposals:
//
//   - definition_overlap: the same symbol (same file, same name) is changed by
//     both — high severity when either removes it or changes its signature;
//   - caller_affected: one proposal removes or re-signs an exported symbol that
//     a file changed by the other mentions;
//   - name_overlap: exported symbols with the same name change in different
//     files (a weak hint of parallel work on one concept).
func ComputeOverlaps(ps []*ProposalSymbols) []Overlap {
	var out []Overlap
	for i, a := range ps {
		for _, b := range ps[i+1:] {
			out = append(out, pairOverlaps(a, b)...)
		}
	}
	rank := map[string]int{"high": 0, "medium": 1, "low": 2}
	sort.SliceStable(out, func(i, j int) bool {
		if rank[out[i].Severity] != rank[out[j].Severity] {
			return rank[out[i].Severity] < rank[out[j].Severity]
		}
		if out[i].A != out[j].A {
			return out[i].A < out[j].A
		}
		if out[i].B != out[j].B {
			return out[i].B < out[j].B
		}
		return out[i].Kind < out[j].Kind
	})
	return out
}

func pairOverlaps(a, b *ProposalSymbols) []Overlap {
	var out []Overlap
	type k struct{ path, name, kind string }
	idxB := map[k]Changed{}
	for _, c := range b.Changed {
		idxB[k{c.Path, c.Qualified(), c.Kind}] = c
	}
	var def []SymbolRef
	severity := "medium"
	for _, ca := range a.Changed {
		if cb, ok := idxB[k{ca.Path, ca.Qualified(), ca.Kind}]; ok {
			def = append(def, SymbolRef{Name: ca.Qualified(), Kind: ca.Kind, PathA: ca.Path, PathB: cb.Path, ChangeA: ca.Change, ChangeB: cb.Change})
			if ca.Change == ChangeRemoved || ca.Change == ChangeSignature || cb.Change == ChangeRemoved || cb.Change == ChangeSignature {
				severity = "high"
			}
		}
	}
	if len(def) > 0 {
		out = append(out, Overlap{A: a.Proposal, B: b.Proposal, Kind: KindDefinition, Severity: severity, Symbols: def,
			Why: fmt.Sprintf("both proposals change %d of the same definition(s); even if the text merges, the behaviour they describe may not", len(def))})
	}
	// caller_affected, both directions
	for _, dir := range [][2]*ProposalSymbols{{a, b}, {b, a}} {
		changer, other := dir[0], dir[1]
		var refs []SymbolRef
		for _, c := range changer.Changed {
			if !c.Exported || (c.Change != ChangeRemoved && c.Change != ChangeSignature) {
				continue
			}
			if other.Mentions[c.Name] && !touchesSame(other, c) {
				refs = append(refs, SymbolRef{Name: c.Qualified(), Kind: c.Kind, PathA: c.Path, ChangeA: c.Change})
			}
		}
		if len(refs) > 0 {
			out = append(out, Overlap{A: changer.Proposal, B: other.Proposal, Kind: KindCaller, Severity: "medium", Symbols: refs,
				Why: fmt.Sprintf("%s changes the signature of, or removes, exported symbol(s) that files changed by %s mention", changer.Proposal, other.Proposal)})
		}
	}
	// name_overlap
	byName := map[string]Changed{}
	for _, c := range b.Changed {
		if c.Exported {
			byName[c.Qualified()] = c
		}
	}
	var names []SymbolRef
	for _, ca := range a.Changed {
		if cb, ok := byName[ca.Qualified()]; ok && ca.Exported && cb.Path != ca.Path {
			names = append(names, SymbolRef{Name: ca.Qualified(), Kind: ca.Kind, PathA: ca.Path, PathB: cb.Path, ChangeA: ca.Change, ChangeB: cb.Change})
		}
	}
	if len(names) > 0 {
		out = append(out, Overlap{A: a.Proposal, B: b.Proposal, Kind: KindName, Severity: "low", Symbols: names,
			Why: "exported symbols with the same name change in different files; this is often parallel work on one concept"})
	}
	return out
}

// touchesSame: the other proposal changes the very definition (that is a
// definition_overlap, reported separately).
func touchesSame(other *ProposalSymbols, c Changed) bool {
	for _, o := range other.Changed {
		if o.Path == c.Path && o.Qualified() == c.Qualified() && o.Kind == c.Kind {
			return true
		}
	}
	return false
}
