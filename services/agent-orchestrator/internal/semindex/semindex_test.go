package semindex

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

const repo = "acme/web"

var skipDefaults = []string{"vendor/**", "**/vendor/**", "node_modules/**", "**/node_modules/**", "**/*.min.js", "**/testdata/**"}

type harness struct {
	t       *testing.T
	fake    *fakeagwp.Server
	client  *agwp.Client
	comp    *Component
	store   *Store
	metrics *metrics.Service
	cfg     config.Repo
	dir     string
}

func newHarness(t *testing.T, mut func(*config.Repo)) *harness {
	t.Helper()
	h := &harness{t: t, fake: fakeagwp.New(), metrics: metrics.NewService(metrics.NewRegistry()), dir: t.TempDir()}
	t.Cleanup(h.fake.Close)
	h.fake.AddRepo(repo)
	tok := h.fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "writer"})
	var err error
	h.client, err = agwp.New(h.fake.URL, secret.New(tok))
	if err != nil {
		t.Fatal(err)
	}
	h.cfg = config.Repo{Name: repo, Stream: "main", Index: config.Index{MaxBytes: 4096, MaxFiles: 100, Skip: skipDefaults}}
	if mut != nil {
		mut(&h.cfg)
	}
	h.store, err = OpenStore(filepath.Join(h.dir, "index"), repo)
	if err != nil {
		t.Fatal(err)
	}
	h.comp = h.newComponent()
	return h
}

func (h *harness) newComponent() *Component {
	c, err := New(Options{Repo: h.cfg, API: h.client, Store: h.store, Metrics: h.metrics, Log: slog.New(slog.NewTextHandler(io.Discard, nil)), Loop: config.Loop{Reconcile: time.Minute}})
	if err != nil {
		h.t.Fatal(err)
	}
	return c
}

func (h *harness) commit(parents []string, files map[string]string) string {
	return h.fake.Commit(repo, parents, files, "user:alice", "c")
}

func (h *harness) fileFetches() int { return h.fake.CountRequests("tree.file") }

var baseFiles = map[string]string{
	"cmd/main.go":          "package main\n\nfunc main() {\n\tserver.Serve()\n}\n",
	"pkg/server/server.go": "package server\n\ntype Server struct{}\n\nfunc New() *Server { return &Server{} }\n\nfunc (s *Server) Serve() error { return nil }\n\nfunc helper() {}\n",
	"src/lib.rs":           "pub fn parse(input: &str) -> u32 { 0 }\n\nfn internal() {}\n",
	"app/util.py":          "MAX = 3\n\ndef parse(text):\n    return text\n\nclass Parser:\n    def run(self):\n        pass\n",
	"web/index.ts":         "export function render(): string {\n  return \"\";\n}\n\nexport class View {\n  show() {}\n}\n",
	"README.md":            "# readme\n",
	"vendor/dep/dep.go":    "package dep\n\nfunc Vendored() {}\n",
	"web/app.min.js":       "function a(){}\n",
	"assets/blob.go":       "package assets\n\nvar data = \"\x00\x01\"\n",
	"gen/huge.go":          "package gen\n\n" + strings.Repeat("// padding padding padding\n", 400) + "func Huge() {}\n",
}

func TestInitialSyncIndexesSupportedFilesAndCountsWhatItSkips(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	st := h.comp.Index().Stats()
	if st.Files != 5 {
		t.Fatalf("indexed files = %d (%v), want the 5 supported source files", st.Files, h.comp.Index().Paths())
	}
	want := Skipped{Pattern: 2, Unsupported: 1, TooLarge: 1, Binary: 1}
	if st.Skipped != want {
		t.Fatalf("skipped = %+v, want %+v", st.Skipped, want)
	}
	if st.Head == "" || st.Symbols < 10 || st.UpdatedAt.IsZero() {
		t.Fatalf("stats = %+v", st)
	}
	if h.metrics.IndexFiles.With(repo).Value() != 5 || h.metrics.IndexUpdates.With(repo).Value() != 1 {
		t.Fatal("index metrics not recorded")
	}
}

func TestSymbolQueries(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	x := h.comp.Index()

	prefix := x.Symbols(Query{Q: "Ser"})
	if len(prefix) != 2 || prefix[0].Name != "Serve" || prefix[1].Name != "Server" {
		t.Fatalf("prefix Ser = %+v", prefix)
	}
	if d := prefix[0].Definition[0]; d.Path != "pkg/server/server.go" || d.Container != "Server" || d.Kind != "method" || !d.Exported || d.Line != 7 {
		t.Fatalf("Serve definition = %+v", d)
	}
	if prefix[0].References != 1 { // cmd/main.go calls server.Serve()
		t.Fatalf("Serve references = %d, want 1", prefix[0].References)
	}
	if exact := x.Symbols(Query{Q: "Server", Exact: true}); len(exact) != 1 || exact[0].Definition[0].Kind != "struct" {
		t.Fatalf("exact Server = %+v", exact)
	}
	if none := x.Symbols(Query{Q: "Serv", Exact: true}); len(none) != 0 {
		t.Fatalf("exact must not match prefixes: %+v", none)
	}
	// the same name in several languages
	parse := x.Symbols(Query{Q: "parse", Exact: true})
	if len(parse) != 1 || len(parse[0].Definition) != 2 {
		t.Fatalf("parse = %+v", parse)
	}
	f := false
	for _, s := range x.Symbols(Query{Q: "", Exported: &f}) {
		for _, d := range s.Definition {
			if d.Exported {
				t.Fatalf("exported filter leaked %+v", d)
			}
		}
	}
	for _, s := range x.Symbols(Query{Q: "", Kind: "class"}) {
		for _, d := range s.Definition {
			if d.Kind != "class" {
				t.Fatalf("kind filter leaked %+v", d)
			}
		}
	}
	if limited := x.Symbols(Query{Q: "", Limit: 3}); len(limited) != 3 {
		t.Fatalf("limit ignored: %d", len(limited))
	}
	// vendored and generated code never shows up
	for _, name := range []string{"Vendored", "Huge", "a"} {
		if len(x.Symbols(Query{Q: name, Exact: true})) != 0 {
			t.Fatalf("%s must not be indexed", name)
		}
	}
}

func TestFilesForASymbol(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	u := h.comp.Index().FilesFor("Serve")
	if len(u.DefinedIn) != 1 || u.DefinedIn[0].Path != "pkg/server/server.go" {
		t.Fatalf("defined in %+v", u.DefinedIn)
	}
	if len(u.ReferencedIn) != 1 || u.ReferencedIn[0].Path != "cmd/main.go" || u.ReferencedIn[0].Count != 1 {
		t.Fatalf("referenced in %+v", u.ReferencedIn)
	}
	if none := h.comp.Index().FilesFor("Nonexistent"); len(none.DefinedIn) != 0 || len(none.ReferencedIn) != 0 {
		t.Fatalf("usage of an unknown symbol = %+v", none)
	}
}

func TestIncrementalSyncFetchesOnlyChangedFiles(t *testing.T) {
	h := newHarness(t, nil)
	first := h.commit(nil, baseFiles)
	h.fake.SetHead(repo, "main", first)
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	fetched := h.fileFetches()

	// unchanged head: no tree walk, no fetches
	if err := h.comp.Sync(context.Background()); err != nil || h.fileFetches() != fetched {
		t.Fatalf("an unchanged head must cost nothing (fetches %d -> %d, err %v)", fetched, h.fileFetches(), err)
	}

	next := map[string]string{}
	for k, v := range baseFiles {
		next[k] = v
	}
	next["pkg/server/server.go"] += "\nfunc Added() {}\n"
	delete(next, "src/lib.rs")
	next["pkg/server/extra.go"] = "package server\n\nfunc Extra() {}\n"
	h.fake.SetHead(repo, "main", h.commit([]string{first}, next))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	// one modified file and one new one; nothing else is fetched. (Files skipped for
	// size or content are looked at again, they are cheap to reject and have no entry.)
	extra := h.fileFetches() - fetched
	if extra > 4 {
		t.Fatalf("fetched %d files for a two-file change", extra)
	}
	x := h.comp.Index()
	if len(x.Symbols(Query{Q: "Added", Exact: true})) != 1 || len(x.Symbols(Query{Q: "Extra", Exact: true})) != 1 {
		t.Fatal("new definitions missing")
	}
	if len(x.Symbols(Query{Q: "internal", Exact: true})) != 0 || len(x.Symbols(Query{Q: "parse"})) != 1 {
		t.Fatalf("removed file's definitions linger: %+v", x.Symbols(Query{Q: "", Limit: 100}))
	}
}

func TestIndexSurvivesARestartWithoutRefetching(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	before := h.comp.Index().Stats()

	again := h.newComponent() // loads from disk
	after := again.Index().Stats()
	if after.Files != before.Files || after.Symbols != before.Symbols || after.Head != before.Head {
		t.Fatalf("restored stats = %+v, want %+v", after, before)
	}
	fetches := h.fileFetches()
	if err := again.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	if h.fileFetches() != fetches {
		t.Fatalf("a restored index at the current head fetched %d files", h.fileFetches()-fetches)
	}
	if len(again.Index().Symbols(Query{Q: "Serve", Exact: true})) != 1 {
		t.Fatal("restored index lost a symbol")
	}
	// files on disk are private
	filepath.WalkDir(filepath.Join(h.dir, "index"), func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() {
			if st, _ := os.Stat(p); st.Mode().Perm() != 0o600 {
				t.Errorf("%s has mode %v", p, st.Mode().Perm())
			}
		}
		return nil
	})
}

func TestCorruptStoredEntriesAreRebuilt(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	var victim string
	filepath.WalkDir(filepath.Join(h.dir, "index"), func(p string, d os.DirEntry, err error) error {
		if err == nil && !d.IsDir() && strings.HasSuffix(p, ".json") && !strings.HasSuffix(p, "head.json") && victim == "" {
			victim = p
		}
		return nil
	})
	if err := os.WriteFile(victim, []byte("{ torn"), 0o600); err != nil {
		t.Fatal(err)
	}
	again := h.newComponent()
	if got := again.Index().Stats().Files; got != 4 {
		t.Fatalf("files after losing one entry = %d", got)
	}
	// the head is considered unchanged, but a different head forces a diff by blob and restores the file
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := again.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	if got := again.Index().Stats().Files; got != 5 {
		t.Fatalf("the lost entry was not rebuilt: %d files", got)
	}
}

func TestLimitsBoundTheIndex(t *testing.T) {
	h := newHarness(t, func(r *config.Repo) { r.Index.MaxFiles = 2 })
	h.fake.SetHead(repo, "main", h.commit(nil, baseFiles))
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	st := h.comp.Index().Stats()
	if st.Files != 2 || st.Skipped.OverLimit != 4 || st.Skipped.Binary != 1 {
		t.Fatalf("stats = %+v", st)
	}
	if got := h.comp.Index().Paths(); got[0] != "app/util.py" || got[1] != "cmd/main.go" {
		t.Fatalf("the first paths in order are kept deterministically: %v", got)
	}
}

func TestFetchErrorsKeepTheOldHeadSoTheyAreRetried(t *testing.T) {
	h := newHarness(t, nil)
	h.fake.SetHead(repo, "main", h.commit(nil, map[string]string{"a.go": "package a\n\nfunc A() {}\n", "b.go": "package b\n\nfunc B() {}\n"}))
	h.fake.FailNext("tree.file", 1, 500, "internal", false, 0)
	if err := h.comp.Sync(context.Background()); err == nil {
		t.Fatal("expected the fetch error")
	}
	st := h.comp.Index().Stats()
	if st.Skipped.Errors != 1 || st.LastError == "" || st.Files != 1 || st.Head != "" {
		t.Fatalf("stats after a partial sync = %+v", st)
	}
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatalf("retry: %v", err)
	}
	if st := h.comp.Index().Stats(); st.Files != 2 || st.Head == "" || st.LastError != "" {
		t.Fatalf("stats after the retry = %+v", st)
	}
}

func TestAnIntegrationEventUpdatesTheIndexThroughTheLoop(t *testing.T) {
	h := newHarness(t, nil)
	first := h.commit(nil, map[string]string{"a.go": "package a\n\nfunc Old() {}\n"})
	h.fake.SetHead(repo, "main", first)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go h.comp.Run(ctx)
	waitFor(t, "the first pass", func() bool { return len(h.comp.Index().Symbols(Query{Q: "Old", Exact: true})) == 1 })
	h.fake.SetHead(repo, "main", h.commit([]string{first}, map[string]string{"a.go": "package a\n\nfunc Old() {}\nfunc BrandNew() {}\n"}))
	h.comp.OnEvent(agwp.Event{Topic: "stream.integrate"})
	waitFor(t, "the new symbol", func() bool { return len(h.comp.Index().Symbols(Query{Q: "BrandNew", Exact: true})) == 1 })
}

func waitFor(t *testing.T, what string, cond func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !cond() {
		if time.Now().After(deadline) {
			t.Fatalf("timed out waiting for %s", what)
		}
		time.Sleep(2 * time.Millisecond)
	}
}

func TestEmptyStreamsAreFine(t *testing.T) {
	h := newHarness(t, nil)
	if err := h.comp.Sync(context.Background()); err != nil {
		t.Fatal(err)
	}
	if st := h.comp.Index().Stats(); st.Files != 0 || st.Head != "" {
		t.Fatalf("stats = %+v", st)
	}
}

// ---- semantic overlap ------------------------------------------------------------------

func overlapHarness(t *testing.T) (*harness, string) {
	h := newHarness(t, nil)
	seed := h.commit(nil, map[string]string{
		"pkg/api/api.go":   "package api\n\nfunc Fetch(id string) string {\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n",
		"pkg/api/other.go": "package api\n\nfunc Unrelated() {}\n",
		"cmd/tool/main.go": "package main\n\nfunc main() {\n\tapi.Fetch(\"x\")\n}\n",
		"pkg/util/util.go": "package util\n\nfunc Fetch(a, b int) int { return a }\n",
	})
	h.fake.SetHead(repo, "main", seed)
	return h, seed
}

func (h *harness) propose(seed, title string, files map[string]string) string {
	snap := map[string]string{
		"pkg/api/api.go":   "package api\n\nfunc Fetch(id string) string {\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n",
		"pkg/api/other.go": "package api\n\nfunc Unrelated() {}\n",
		"cmd/tool/main.go": "package main\n\nfunc main() {\n\tapi.Fetch(\"x\")\n}\n",
		"pkg/util/util.go": "package util\n\nfunc Fetch(a, b int) int { return a }\n",
	}
	for k, v := range files {
		snap[k] = v
	}
	head := h.commit([]string{seed}, snap)
	return h.fake.AddProposal(repo, "agent:bot#r", "main", head, title, "")
}

func TestProposalsChangingTheSameFunctionOverlapEvenWhenTheTextMerges(t *testing.T) {
	h, seed := overlapHarness(t)
	// Both edit Fetch in api.go (different lines of the body: git would merge it) ...
	a := h.propose(seed, "A: validate", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\tif id == \"\" {\n\t\treturn \"\"\n\t}\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	b := h.propose(seed, "B: log", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\tprintln(id)\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	// ... a third proposal only touches an unrelated function in the same package
	c := h.propose(seed, "C: unrelated", map[string]string{"pkg/api/other.go": "package api\n\nfunc Unrelated() { println() }\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	ov := h.comp.Overlaps()
	var def *Overlap
	for i := range ov {
		if ov[i].Kind == KindDefinition {
			def = &ov[i]
		}
	}
	if def == nil {
		t.Fatalf("overlaps = %+v", ov)
	}
	ids := []string{def.A, def.B}
	if !contains(ids, a) || !contains(ids, b) || contains(ids, c) {
		t.Fatalf("the overlap must be between A and B only: %+v", def)
	}
	if len(def.Symbols) != 1 || def.Symbols[0].Name != "Fetch" || def.Symbols[0].ChangeA != ChangeModified || def.Severity != "medium" {
		t.Fatalf("symbols = %+v severity %s", def.Symbols, def.Severity)
	}
	for _, o := range ov {
		if contains([]string{o.A, o.B}, c) {
			t.Fatalf("C touches different definitions and must not overlap with anything: %+v", o)
		}
	}
}

func TestChangingASignatureIsHighSeverityAndAffectsCallers(t *testing.T) {
	h, seed := overlapHarness(t)
	changer := h.propose(seed, "re-sign Fetch", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string, opts Options) string {\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	caller := h.propose(seed, "use Fetch", map[string]string{"cmd/tool/main.go": "package main\n\nfunc main() {\n\tapi.Fetch(\"x\")\n\tapi.Fetch(\"y\")\n}\n"})
	same := h.propose(seed, "edit Fetch too", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id + \"!\"\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	kinds := map[string]Overlap{}
	for _, o := range h.comp.Overlaps() {
		kinds[o.Kind+":"+o.A+":"+o.B] = o
	}
	var haveCaller, haveHigh bool
	for _, o := range h.comp.Overlaps() {
		if o.Kind == KindCaller && o.A == changer && o.B == caller {
			haveCaller = true
			if o.Symbols[0].Name != "Fetch" || o.Symbols[0].ChangeA != ChangeSignature {
				t.Fatalf("caller overlap = %+v", o)
			}
		}
		if o.Kind == KindDefinition && contains([]string{o.A, o.B}, changer) && contains([]string{o.A, o.B}, same) {
			haveHigh = o.Severity == "high"
		}
	}
	if !haveCaller {
		t.Fatalf("a signature change must flag proposals whose files mention the symbol: %+v", kinds)
	}
	if !haveHigh {
		t.Fatalf("a signature change colliding with an edit of the same function is high severity: %+v", kinds)
	}
	// the highest severity comes first
	if ov := h.comp.Overlaps(); ov[0].Severity != "high" {
		t.Fatalf("ordering: %+v", ov[0])
	}
}

func TestSameNamedExportsInDifferentFilesAreALowSeverityHint(t *testing.T) {
	h, seed := overlapHarness(t)
	h.propose(seed, "api Fetch", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id + \"1\"\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	h.propose(seed, "util Fetch", map[string]string{"pkg/util/util.go": "package util\n\nfunc Fetch(a, b int) int { return b }\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	ov := h.comp.Overlaps()
	if len(ov) != 1 || ov[0].Kind != KindName || ov[0].Severity != "low" || ov[0].Symbols[0].PathA == ov[0].Symbols[0].PathB {
		t.Fatalf("overlaps = %+v", ov)
	}
}

func TestDisjointProposalsDoNotOverlap(t *testing.T) {
	h, seed := overlapHarness(t)
	h.propose(seed, "Store", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id\n}\n\nfunc Store(v string) {\n\t_ = v + \"x\"\n}\n"})
	h.propose(seed, "Unrelated", map[string]string{"pkg/api/other.go": "package api\n\nfunc Unrelated() { _ = 1 }\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if ov := h.comp.Overlaps(); len(ov) != 0 {
		t.Fatalf("overlaps = %+v", ov)
	}
}

func TestAddedAndRemovedDefinitionsAreClassified(t *testing.T) {
	h, seed := overlapHarness(t)
	id := h.propose(seed, "add and remove", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id\n}\n\nfunc Brand() {}\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	var ps *ProposalSymbols
	for _, p := range h.comp.Changes() {
		if p.Proposal == id {
			ps = p
		}
	}
	if ps == nil {
		t.Fatal("proposal not analysed")
	}
	got := map[string]string{}
	for _, c := range ps.Changed {
		got[c.Name] = c.Change
	}
	if got["Brand"] != ChangeAdded || got["Store"] != ChangeRemoved || got["Fetch"] != "" {
		t.Fatalf("changed = %v", got)
	}
	if len(ps.Files) != 1 || ps.Files[0] != "pkg/api/api.go" {
		t.Fatalf("files = %v", ps.Files)
	}
}

func TestAnalysesFollowProposalHeadsAndDisappearWithTheProposal(t *testing.T) {
	h, seed := overlapHarness(t)
	id := h.propose(seed, "x", map[string]string{"pkg/api/other.go": "package api\n\nfunc Unrelated() { _ = 1 }\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(h.comp.Changes()) != 1 {
		t.Fatal("not analysed")
	}
	calls := h.fake.CountRequests("tree.diff")
	if err := h.comp.Reconcile(context.Background()); err != nil || h.fake.CountRequests("tree.diff") != calls {
		t.Fatal("an unchanged proposal head must not be analysed again")
	}
	h.fake.CloseProposalFixture(repo, id)
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	if len(h.comp.Changes()) != 0 {
		t.Fatal("closed proposals must be forgotten")
	}
}

func contains(l []string, s string) bool {
	for _, x := range l {
		if x == s {
			return true
		}
	}
	return false
}

// ---- HTTP -------------------------------------------------------------------------------

func get(t *testing.T, ts *httptest.Server, path string) (int, map[string]any) {
	t.Helper()
	resp, err := http.Get(ts.URL + path)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var out map[string]any
	_ = json.NewDecoder(resp.Body).Decode(&out)
	return resp.StatusCode, out
}

func TestHTTPQueryAPI(t *testing.T) {
	h, seed := overlapHarness(t)
	h.propose(seed, "A", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id + \"a\"\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	h.propose(seed, "B", map[string]string{"pkg/api/api.go": "package api\n\nfunc Fetch(id string) string {\n\treturn id + \"b\"\n}\n\nfunc Store(v string) {\n\t_ = v\n}\n"})
	if err := h.comp.Reconcile(context.Background()); err != nil {
		t.Fatal(err)
	}
	reg := NewRegistry()
	reg.Add(h.comp)
	ts := httptest.NewServer(reg.Handler())
	defer ts.Close()

	code, body := get(t, ts, "/index/symbols?repo=acme/web&q=Fet")
	syms := body["symbols"].([]any)
	if code != 200 || len(syms) != 1 || syms[0].(map[string]any)["name"] != "Fetch" || len(syms[0].(map[string]any)["definitions"].([]any)) != 2 {
		t.Fatalf("symbols: %d %v", code, body)
	}
	// the lone repository is the default
	if code, body = get(t, ts, "/index/symbols?q=Store&mode=exact"); code != 200 || len(body["symbols"].([]any)) != 1 {
		t.Fatalf("default repo: %d %v", code, body)
	}
	if code, body = get(t, ts, "/index/files?symbol=Fetch"); code != 200 {
		t.Fatalf("files: %d %v", code, body)
	} else {
		usage := body["usage"].(map[string]any)
		if len(usage["defined_in"].([]any)) != 2 || len(usage["referenced_in"].([]any)) == 0 {
			t.Fatalf("usage = %v", usage)
		}
	}
	if code, body = get(t, ts, "/index/overlaps"); code != 200 || body["proposals_analysed"] != float64(2) || len(body["overlaps"].([]any)) == 0 {
		t.Fatalf("overlaps: %d %v", code, body)
	}
	if code, body = get(t, ts, "/index/changes"); code != 200 || len(body["proposals"].([]any)) != 2 {
		t.Fatalf("changes: %d %v", code, body)
	}
	if code, _ = get(t, ts, "/index/changes?proposal=prop_nope"); code != 404 {
		t.Fatalf("unknown proposal: %d", code)
	}
	if code, body = get(t, ts, "/index/status"); code != 200 || len(body["repos"].([]any)) != 1 {
		t.Fatalf("status: %d %v", code, body)
	}
	if code, body = get(t, ts, "/index/status?repo=acme/web"); code != 200 || body["repo"] != "acme/web" {
		t.Fatalf("repo status: %d %v", code, body)
	}

	for path, want := range map[string]int{
		"/index/symbols?repo=nope/nope&q=x": 404,
		"/index/symbols?q=x&mode=fuzzy":     400,
		"/index/symbols?mode=exact":         400,
		"/index/symbols?q=x&exported=maybe": 400,
		"/index/symbols?q=x&limit=0":        400,
		"/index/symbols?q=x&limit=abc":      400,
		"/index/files":                      400,
		"/index/status?repo=nope/nope":      404,
	} {
		code, body := get(t, ts, path)
		if code != want {
			t.Errorf("%s: %d, want %d", path, code, want)
			continue
		}
		if e, ok := body["error"].(map[string]any); !ok || e["code"] == "" || e["message"] == "" {
			t.Errorf("%s: error body %v", path, body)
		}
	}
	resp, _ := http.Post(ts.URL+"/index/symbols?q=x", "application/json", strings.NewReader("{}"))
	if resp.StatusCode != http.StatusMethodNotAllowed {
		t.Errorf("POST: %d", resp.StatusCode)
	}
	resp.Body.Close()
}

func TestHTTPRequiresARepoWhenThereAreSeveral(t *testing.T) {
	h := newHarness(t, nil)
	other := newHarness(t, func(r *config.Repo) { r.Name = "acme/api" })
	reg := NewRegistry()
	reg.Add(h.comp)
	reg.Add(other.comp)
	ts := httptest.NewServer(reg.Handler())
	defer ts.Close()
	if code, _ := get(t, ts, "/index/symbols?q=x"); code != 400 {
		t.Fatalf("ambiguous repo: %d", code)
	}
	if len(reg.Statuses()) != 2 {
		t.Fatal("Statuses")
	}
}

func TestCompareDefsDetectsEditsInsideSpans(t *testing.T) {
	old := "package a\n\nfunc A() {\n\tx := 1\n\t_ = x\n}\n\nfunc B() {}\n"
	neu := "package a\n\nfunc A() {\n\tx := 2\n\t_ = x\n}\n\nfunc B() {}\n"
	oldS, newS := Extract(LangGo, []byte(old), nil), Extract(LangGo, []byte(neu), nil)
	got := compareDefs("a.go", []byte(old), []byte(neu), oldS.Defs, newS.Defs)
	if len(got) != 1 || got[0].Name != "A" || got[0].Change != ChangeModified {
		t.Fatalf("changes = %+v", got)
	}
	if fmt.Sprint(compareDefs("a.go", []byte(old), []byte(old), oldS.Defs, oldS.Defs)) != "[]" {
		t.Fatal("identical files must not report changes")
	}
}
