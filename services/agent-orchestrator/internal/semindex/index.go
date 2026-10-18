package semindex

import (
	"sort"
	"strings"
	"sync"
	"time"
)

// FileEntry is what the index knows about one file at one blob.
type FileEntry struct {
	Path string  `json:"path"`
	Blob string  `json:"blob"`
	Syms Symbols `json:"symbols"`
}

// DefRef is a definition with the file it lives in.
type DefRef struct {
	Path string `json:"path"`
	Def
}

// Skipped counts files left out of the index and why. The index is bounded
// by design, so a sceptical reader can see what it does not cover.
type Skipped struct {
	TooLarge    int `json:"too_large"`
	Binary      int `json:"binary"`
	Pattern     int `json:"pattern"`
	Unsupported int `json:"unsupported_language"`
	OverLimit   int `json:"over_file_limit"`
	Errors      int `json:"fetch_errors"`
}

// Stats summarises the index for /status.
type Stats struct {
	Repo      string    `json:"repo"`
	Head      string    `json:"head"`
	Files     int       `json:"files"`
	Symbols   int       `json:"symbols"`
	Names     int       `json:"distinct_names"`
	UpdatedAt time.Time `json:"updated_at,omitempty"`
	Skipped   Skipped   `json:"skipped"`
	// DroppedDefs and DroppedIdents are the definitions and identifiers left out by the bounds of a file or of the
	// whole index: symbol queries and reference counts are incomplete by that much.
	DroppedDefs   int    `json:"dropped_definitions"`
	DroppedIdents int    `json:"dropped_identifiers"`
	LastError     string `json:"last_error,omitempty"`
}

// Index is the in-memory symbol table of one repository's stream head. It is
// safe for concurrent use.
type Index struct {
	repo string

	mu      sync.RWMutex
	files   map[string]*FileEntry
	byName  map[string][]DefRef
	names   []string // sorted distinct names
	refs    map[string]int
	head    string
	updated time.Time
	skipped Skipped
	lastErr string
	symbols int

	droppedDefs, droppedIdents int
}

// NewIndex returns an empty index.
func NewIndex(repo string) *Index {
	return &Index{repo: repo, files: map[string]*FileEntry{}, byName: map[string][]DefRef{}, refs: map[string]int{}}
}

// Known returns the set of names currently defined (for identifier budgeting).
func (x *Index) Known() map[string]bool {
	x.mu.RLock()
	defer x.mu.RUnlock()
	out := make(map[string]bool, len(x.byName))
	for n := range x.byName {
		out[n] = true
	}
	return out
}

// Entry returns the entry for a path, if any.
func (x *Index) Entry(path string) (*FileEntry, bool) {
	x.mu.RLock()
	defer x.mu.RUnlock()
	e, ok := x.files[path]
	return e, ok
}

// Paths returns every indexed path, sorted.
func (x *Index) Paths() []string {
	x.mu.RLock()
	defer x.mu.RUnlock()
	out := make([]string, 0, len(x.files))
	for p := range x.files {
		out = append(out, p)
	}
	sort.Strings(out)
	return out
}

// apply replaces and removes entries, then rebuilds the derived tables once.
func (x *Index) apply(put []*FileEntry, remove []string, head string, skipped Skipped, err error) {
	x.mu.Lock()
	defer x.mu.Unlock()
	for _, p := range remove {
		delete(x.files, p)
	}
	for _, e := range put {
		x.files[e.Path] = e
	}
	x.head, x.skipped, x.updated = head, skipped, time.Now()
	x.lastErr = ""
	if err != nil {
		x.lastErr = err.Error()
	}
	x.rebuildLocked()
}

// maxSymbols bounds the derived tables: beyond it, further definitions are not indexed.
const maxSymbols = 200_000

func (x *Index) rebuildLocked() {
	x.byName = map[string][]DefRef{}
	x.refs = map[string]int{}
	x.symbols, x.droppedDefs, x.droppedIdents = 0, 0, 0
	paths := make([]string, 0, len(x.files))
	for p := range x.files {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	for _, p := range paths {
		e := x.files[p]
		x.droppedDefs += e.Syms.DroppedDefs
		x.droppedIdents += e.Syms.DroppedIdents
		for i, d := range e.Syms.Defs {
			if x.symbols >= maxSymbols {
				x.droppedDefs += len(e.Syms.Defs) - i
				break
			}
			x.byName[d.Name] = append(x.byName[d.Name], DefRef{Path: p, Def: d})
			x.symbols++
		}
		for id, n := range e.Syms.Idents {
			x.refs[id] += int(n)
		}
	}
	x.names = x.names[:0]
	for n := range x.byName {
		x.names = append(x.names, n)
	}
	sort.Strings(x.names)
}

// Query selects symbols.
type Query struct {
	Q        string // name prefix (or the whole name when Exact)
	Exact    bool
	Kind     string // optional filter
	Exported *bool  // optional filter
	Limit    int
}

// Symbol is a query hit.
type Symbol struct {
	Name       string   `json:"name"`
	Definition []DefRef `json:"definitions"`
	// References is the heuristic count of identifier occurrences across the
	// indexed files, minus the definitions themselves.
	References int `json:"references"`
}

// Symbols returns the matching names in alphabetical order.
func (x *Index) Symbols(q Query) []Symbol {
	x.mu.RLock()
	defer x.mu.RUnlock()
	if q.Limit <= 0 || q.Limit > 500 {
		q.Limit = 50
	}
	var out []Symbol
	add := func(name string) bool {
		var defs []DefRef
		for _, d := range x.byName[name] {
			if q.Kind != "" && d.Kind != q.Kind {
				continue
			}
			if q.Exported != nil && d.Exported != *q.Exported {
				continue
			}
			defs = append(defs, d)
		}
		if len(defs) == 0 {
			return true
		}
		out = append(out, Symbol{Name: name, Definition: defs, References: max(0, x.refs[name]-len(x.byName[name]))})
		return len(out) < q.Limit
	}
	if q.Exact {
		if _, ok := x.byName[q.Q]; ok {
			add(q.Q)
		}
		return out
	}
	i := sort.SearchStrings(x.names, q.Q)
	for ; i < len(x.names) && strings.HasPrefix(x.names[i], q.Q); i++ {
		if !add(x.names[i]) {
			break
		}
	}
	return out
}

// FileRef is a file that mentions a symbol.
type FileRef struct {
	Path  string `json:"path"`
	Count int    `json:"count"`
}

// Usage says where a symbol is defined and where it is mentioned.
type Usage struct {
	Symbol       string    `json:"symbol"`
	DefinedIn    []DefRef  `json:"defined_in"`
	ReferencedIn []FileRef `json:"referenced_in"`
}

// FilesFor returns the files that define or mention a symbol.
func (x *Index) FilesFor(symbol string) Usage {
	x.mu.RLock()
	defer x.mu.RUnlock()
	u := Usage{Symbol: symbol, DefinedIn: append([]DefRef(nil), x.byName[symbol]...)}
	for p, e := range x.files {
		n := int(e.Syms.Idents[symbol])
		for _, d := range x.byName[symbol] {
			if d.Path == p {
				n--
			}
		}
		if n > 0 {
			u.ReferencedIn = append(u.ReferencedIn, FileRef{Path: p, Count: n})
		}
	}
	sort.Slice(u.ReferencedIn, func(i, j int) bool {
		if u.ReferencedIn[i].Count != u.ReferencedIn[j].Count {
			return u.ReferencedIn[i].Count > u.ReferencedIn[j].Count
		}
		return u.ReferencedIn[i].Path < u.ReferencedIn[j].Path
	})
	return u
}

// Stats returns a summary.
func (x *Index) Stats() Stats {
	x.mu.RLock()
	defer x.mu.RUnlock()
	return Stats{Repo: x.repo, Head: x.head, Files: len(x.files), Symbols: x.symbols, Names: len(x.names), UpdatedAt: x.updated, Skipped: x.skipped,
		DroppedDefs: x.droppedDefs, DroppedIdents: x.droppedIdents, LastError: x.lastErr}
}

// Head is the stream head the index describes.
func (x *Index) Head() string {
	x.mu.RLock()
	defer x.mu.RUnlock()
	return x.head
}
