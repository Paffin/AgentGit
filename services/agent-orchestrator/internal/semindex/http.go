package semindex

import (
	"encoding/json"
	"net/http"
	"sort"
	"strconv"
	"strings"
)

// Registry serves the query API for every repository that has an index.
//
//	GET /index/symbols?repo=&q=&mode=prefix|exact&kind=&exported=&limit=
//	GET /index/files?repo=&symbol=
//	GET /index/overlaps?repo=
//	GET /index/changes?repo=[&proposal=]
//	GET /index/status[?repo=]
//
// The answers are advisory (see the package comment). Authentication, when
// configured, is applied by the caller around Handler.
type Registry struct {
	repos map[string]*Component
}

// NewRegistry returns an empty registry.
func NewRegistry() *Registry { return &Registry{repos: map[string]*Component{}} }

// Add registers a repository's index.
func (r *Registry) Add(c *Component) { r.repos[c.Repo()] = c }

// Handler returns the HTTP handler for the /index/ routes.
func (r *Registry) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("/index/symbols", r.symbols)
	mux.HandleFunc("/index/files", r.files)
	mux.HandleFunc("/index/overlaps", r.overlaps)
	mux.HandleFunc("/index/changes", r.changes)
	mux.HandleFunc("/index/status", r.status)
	return mux
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	_ = enc.Encode(v)
}

func httpError(w http.ResponseWriter, status int, code, msg string) {
	writeJSON(w, status, map[string]any{"error": map[string]any{"code": code, "message": msg}})
}

// pick resolves the repo query parameter; a lone repository is the default.
func (r *Registry) pick(w http.ResponseWriter, req *http.Request) *Component {
	if req.Method != http.MethodGet && req.Method != http.MethodHead {
		httpError(w, http.StatusMethodNotAllowed, "invalid_argument", "only GET is supported")
		return nil
	}
	name := req.URL.Query().Get("repo")
	if name == "" && len(r.repos) == 1 {
		for _, c := range r.repos {
			return c
		}
	}
	if name == "" {
		httpError(w, http.StatusBadRequest, "invalid_argument", "the repo parameter (owner/name) is required")
		return nil
	}
	c := r.repos[name]
	if c == nil {
		httpError(w, http.StatusNotFound, "not_found", "no index for repository "+name)
		return nil
	}
	return c
}

func (r *Registry) symbols(w http.ResponseWriter, req *http.Request) {
	c := r.pick(w, req)
	if c == nil {
		return
	}
	q := req.URL.Query()
	query := Query{Q: q.Get("q"), Kind: q.Get("kind")}
	switch q.Get("mode") {
	case "", "prefix":
	case "exact":
		query.Exact = true
	default:
		httpError(w, http.StatusBadRequest, "invalid_argument", "mode must be prefix or exact")
		return
	}
	if query.Exact && query.Q == "" {
		httpError(w, http.StatusBadRequest, "invalid_argument", "q is required for mode=exact")
		return
	}
	if v := q.Get("exported"); v != "" {
		b, err := strconv.ParseBool(v)
		if err != nil {
			httpError(w, http.StatusBadRequest, "invalid_argument", "exported must be true or false")
			return
		}
		query.Exported = &b
	}
	if v := q.Get("limit"); v != "" {
		n, err := strconv.Atoi(v)
		if err != nil || n < 1 {
			httpError(w, http.StatusBadRequest, "invalid_argument", "limit must be a positive integer")
			return
		}
		query.Limit = n
	}
	syms := c.idx.Symbols(query)
	if syms == nil {
		syms = []Symbol{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"repo": c.Repo(), "head": c.idx.Head(), "symbols": syms})
}

func (r *Registry) files(w http.ResponseWriter, req *http.Request) {
	c := r.pick(w, req)
	if c == nil {
		return
	}
	sym := strings.TrimSpace(req.URL.Query().Get("symbol"))
	if sym == "" {
		httpError(w, http.StatusBadRequest, "invalid_argument", "the symbol parameter is required")
		return
	}
	u := c.idx.FilesFor(sym)
	if u.DefinedIn == nil {
		u.DefinedIn = []DefRef{}
	}
	if u.ReferencedIn == nil {
		u.ReferencedIn = []FileRef{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"repo": c.Repo(), "head": c.idx.Head(), "usage": u})
}

func (r *Registry) overlaps(w http.ResponseWriter, req *http.Request) {
	c := r.pick(w, req)
	if c == nil {
		return
	}
	ov := c.Overlaps()
	if ov == nil {
		ov = []Overlap{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"repo": c.Repo(), "proposals_analysed": len(c.Changes()), "overlaps": ov})
}

func (r *Registry) changes(w http.ResponseWriter, req *http.Request) {
	c := r.pick(w, req)
	if c == nil {
		return
	}
	only := req.URL.Query().Get("proposal")
	out := []*ProposalSymbols{}
	for _, ps := range c.Changes() {
		if only == "" || ps.Proposal == only {
			out = append(out, ps)
		}
	}
	if only != "" && len(out) == 0 {
		httpError(w, http.StatusNotFound, "not_found", "no analysis for proposal "+only)
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"repo": c.Repo(), "proposals": out})
}

func (r *Registry) status(w http.ResponseWriter, req *http.Request) {
	if req.Method != http.MethodGet && req.Method != http.MethodHead {
		httpError(w, http.StatusMethodNotAllowed, "invalid_argument", "only GET is supported")
		return
	}
	if name := req.URL.Query().Get("repo"); name != "" {
		c := r.repos[name]
		if c == nil {
			httpError(w, http.StatusNotFound, "not_found", "no index for repository "+name)
			return
		}
		writeJSON(w, http.StatusOK, c.Status())
		return
	}
	names := make([]string, 0, len(r.repos))
	for n := range r.repos {
		names = append(names, n)
	}
	sort.Strings(names)
	out := make([]Status, 0, len(names))
	for _, n := range names {
		out = append(out, r.repos[n].Status())
	}
	writeJSON(w, http.StatusOK, map[string]any{"repos": out})
}

// Statuses returns every repository's status (for /status).
func (r *Registry) Statuses() []Status {
	names := make([]string, 0, len(r.repos))
	for n := range r.repos {
		names = append(names, n)
	}
	sort.Strings(names)
	out := make([]Status, 0, len(names))
	for _, n := range names {
		out = append(out, r.repos[n].Status())
	}
	return out
}
