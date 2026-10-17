// Package advisor explains merge conflicts and, where a safe mechanical
// resolution exists, suggests one.
//
// A suggestion is advice. It is computed deterministically from three file
// versions, posted as a "comment" review (never an approval or a request for
// changes) and kept in the advisor's own status API. Nothing here is trusted
// or applied automatically: a resolution only enters the repository when an
// agent or a person submits it through a virtual workspace sync with
// "resolutions", and the stream policy still decides whether the result lands.
package advisor

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"path"
	"regexp"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/textdiff"
)

// Advisor turns one conflicting file into a suggestion. The rule-based
// implementation below is the one that ships; an implementation backed by a
// language model could be plugged in behind the same interface, and its output
// would have exactly the same standing: advice that a person or agent may
// accept, nothing the system applies.
type Advisor interface {
	// Name identifies the advisor in comments and metrics ("rules/1").
	Name() string
	// Suggest analyses one conflicting path. It must be deterministic for
	// rule-based advisors and must never have side effects on the repository.
	Suggest(ctx context.Context, in Input) (Suggestion, error)
}

// File is one version of a file. A nil *File means the path does not exist
// on that side.
type File struct {
	Content []byte
}

// Input is a conflicting path with its three versions: Base (the merge base),
// Ours (the stream's current head) and Theirs (the proposal's head).
type Input struct {
	Path   string
	Kind   string // the server's conflict description
	Base   *File
	Ours   *File
	Theirs *File
}

// Classes of regions and files.
const (
	ClassWhitespace  = "whitespace_only"
	ClassUnion       = "union"
	ClassAdjacent    = "adjacent_edits"
	ClassSameLines   = "both_added_same_lines"
	ClassNeedsPerson = "needs_a_person"
	ClassBinary      = "unsupported"
	ClassDeleteEdit  = "delete_modify"
	ClassBothAdded   = "both_added"
	ClassMixed       = "mixed"
)

// Confidence levels. They describe how mechanical the resolution is, not
// whether it is correct: only validation and review can say that.
const (
	High   = "high"
	Medium = "medium"
	Low    = "low"
)

// Region is one conflicting region and what the rules make of it.
type Region struct {
	// BaseLines are 1-based, inclusive line numbers in the merge base
	// (an insertion has Start > End).
	BaseStart int    `json:"base_start"`
	BaseEnd   int    `json:"base_end"`
	Class     string `json:"class"`
	// Resolution is "ours", "theirs", "both", "union" or "none".
	Resolution string `json:"resolution"`
	Confidence string `json:"confidence,omitempty"`
	Why        string `json:"why"`
	Ours       string `json:"ours,omitempty"`
	Theirs     string `json:"theirs,omitempty"`
	Proposed   string `json:"proposed,omitempty"`
}

// Suggestion is the advisor's answer for one path.
type Suggestion struct {
	Path    string   `json:"path"`
	Kind    string   `json:"kind"`
	Advisor string   `json:"advisor"`
	Class   string   `json:"class"`
	Regions []Region `json:"regions,omitempty"`
	// Complete is true when every region has a mechanical resolution; then
	// Resolved holds the whole suggested file.
	Complete   bool     `json:"complete"`
	Confidence string   `json:"confidence,omitempty"`
	Resolved   []byte   `json:"resolved,omitempty"`
	SHA256     string   `json:"resolved_sha256,omitempty"`
	Notes      []string `json:"notes,omitempty"`
}

// Rules is the deterministic, rule-based advisor.
type Rules struct {
	// MaxFileBytes: larger files are not analysed (default 1 MiB).
	MaxFileBytes int
}

// Name implements Advisor.
func (Rules) Name() string { return "rules/1" }

// Suggest implements Advisor.
func (r Rules) Suggest(_ context.Context, in Input) (Suggestion, error) {
	max := r.MaxFileBytes
	if max <= 0 {
		max = 1 << 20
	}
	s := Suggestion{Path: in.Path, Kind: in.Kind, Advisor: r.Name()}
	for _, f := range []*File{in.Base, in.Ours, in.Theirs} {
		if f != nil && !isText(f.Content, max) {
			s.Class = ClassBinary
			s.Notes = append(s.Notes, fmt.Sprintf("binary content, invalid UTF-8 or larger than %d bytes: not analysed", max))
			return s, nil
		}
	}
	switch {
	case in.Ours == nil && in.Theirs == nil:
		s.Class = ClassNeedsPerson
		s.Notes = append(s.Notes, "both sides removed the path; there is nothing to resolve")
	case in.Ours == nil || in.Theirs == nil:
		s.Class = ClassDeleteEdit
		side := "the stream"
		if in.Ours != nil {
			side = "the proposal"
		}
		s.Notes = append(s.Notes, fmt.Sprintf("%s deleted this file while the other side changed it: keeping or dropping the file is a decision for a person", side))
	case in.Base == nil:
		r.bothAdded(&s, in)
	default:
		r.threeWay(&s, in)
	}
	if s.Complete && len(s.Resolved) > 0 {
		sum := sha256.Sum256(s.Resolved)
		s.SHA256 = hex.EncodeToString(sum[:])
	}
	return s, nil
}

func isText(b []byte, max int) bool {
	if len(b) > max {
		return false
	}
	for _, c := range b {
		if c == 0 {
			return false
		}
	}
	return utf8.Valid(b)
}

// ---- both sides added the path ------------------------------------------------------

func (r Rules) bothAdded(s *Suggestion, in Input) {
	ours, theirs := string(in.Ours.Content), string(in.Theirs.Content)
	s.Class = ClassBothAdded
	reg := Region{BaseStart: 1, BaseEnd: 0, Ours: clip(ours), Theirs: clip(theirs)}
	switch {
	case wsEqual(ours, theirs):
		reg.Class, reg.Resolution, reg.Confidence = ClassWhitespace, "ours", High
		reg.Why = "both sides added the same file and differ only in whitespace; keeping the stream's formatting"
		reg.Proposed = clip(ours)
		s.Resolved = []byte(ours)
	case allListLike(in.Path, ours) && allListLike(in.Path, theirs):
		merged := unionLines(textdiff.SplitLines(ours), textdiff.SplitLines(theirs))
		reg.Class, reg.Resolution, reg.Confidence = ClassUnion, "union", High
		reg.Why = "both sides created the same kind of list file; every entry from either side is kept once"
		reg.Proposed = clip(textdiff.Join(merged))
		s.Resolved = []byte(textdiff.Join(merged))
	default:
		reg.Class, reg.Resolution = ClassNeedsPerson, "none"
		reg.Why = "both sides created this file with different content"
		s.Regions = []Region{reg}
		return
	}
	s.Regions = []Region{reg}
	s.Complete, s.Confidence = true, reg.Confidence
	s.Class = reg.Class
}

// ---- three-way ---------------------------------------------------------------------

func (r Rules) threeWay(s *Suggestion, in Input) {
	base := textdiff.SplitLines(string(in.Base.Content))
	ours := textdiff.SplitLines(string(in.Ours.Content))
	theirs := textdiff.SplitLines(string(in.Theirs.Content))
	chunks := textdiff.Merge3(base, ours, theirs)

	resolutions := map[int][]string{} // by chunk index
	for i, c := range chunks {
		if c.Kind != textdiff.Conflict {
			continue
		}
		reg, lines := classifyChunk(in.Path, c)
		s.Regions = append(s.Regions, reg)
		if reg.Resolution != "none" {
			resolutions[i] = lines
		}
	}
	if len(s.Regions) == 0 {
		// The server reported a conflict but the three versions merge cleanly
		// (for example the stream moved since the preparation). Say so.
		s.Class = ClassMixed
		s.Notes = append(s.Notes, "the three versions merge cleanly now; the stream probably moved since the conflict was reported")
		merged, _ := textdiff.Resolve(chunks, func(textdiff.Chunk) ([]string, bool) { return nil, false })
		s.Complete, s.Confidence, s.Resolved = true, High, []byte(textdiff.Join(merged))
		return
	}
	var order []int // chunk indices of the conflicts, in the order Resolve asks about them
	for i, c := range chunks {
		if c.Kind == textdiff.Conflict {
			order = append(order, i)
		}
	}
	asked := 0
	merged, all := textdiff.Resolve(chunks, func(textdiff.Chunk) ([]string, bool) {
		lines, ok := resolutions[order[asked]]
		asked++
		return lines, ok
	})
	classes := map[string]bool{}
	conf := High
	for _, reg := range s.Regions {
		classes[reg.Class] = true
		conf = minConfidence(conf, reg.Confidence)
	}
	switch {
	case len(classes) == 1:
		for c := range classes {
			s.Class = c
		}
	default:
		s.Class = ClassMixed
	}
	if all {
		s.Complete, s.Confidence, s.Resolved = true, conf, []byte(textdiff.Join(merged))
		return
	}
	resolved := 0
	for _, reg := range s.Regions {
		if reg.Resolution != "none" {
			resolved++
		}
	}
	s.Notes = append(s.Notes, fmt.Sprintf("%d of %d conflicting region(s) have a mechanical resolution; the rest need a person", resolved, len(s.Regions)))
}

func minConfidence(a, b string) string {
	rank := map[string]int{High: 3, Medium: 2, Low: 1, "": 3}
	if rank[b] < rank[a] {
		return b
	}
	return a
}

func clip(s string) string {
	const max = 600
	if len(s) <= max {
		return s
	}
	cut := max
	for cut > 0 && !utf8.RuneStart(s[cut]) {
		cut--
	}
	return s[:cut] + "…\n"
}

// classifyChunk applies the region rules in order and returns the region and
// the lines that would replace it.
func classifyChunk(file string, c textdiff.Chunk) (Region, []string) {
	reg := Region{
		BaseStart: c.BaseStart + 1, BaseEnd: c.BaseEnd,
		Ours: clip(textdiff.Join(c.Ours)), Theirs: clip(textdiff.Join(c.Theirs)),
	}
	oursText, theirsText, baseText := textdiff.Join(c.Ours), textdiff.Join(c.Theirs), textdiff.Join(c.Base)

	// 1. whitespace only
	switch {
	case wsEqual(oursText, baseText) && !wsEqual(theirsText, baseText):
		return resolved(reg, ClassWhitespace, "theirs", High,
			"the stream only re-formatted these lines, the proposal changed their content; keeping the proposal's content", c.Theirs)
	case wsEqual(theirsText, baseText) && !wsEqual(oursText, baseText):
		return resolved(reg, ClassWhitespace, "ours", High,
			"the proposal only re-formatted these lines, the stream changed their content; keeping the stream's content", c.Ours)
	case wsEqual(oursText, theirsText):
		return resolved(reg, ClassWhitespace, "ours", High,
			"both sides made the same change and differ only in whitespace; keeping the stream's formatting", c.Ours)
	}

	// 2. import / dependency lists: both sides only add entries
	if onlyInsertions(c.OursHunks) && onlyInsertions(c.TheirsHunks) && insertionsAreListEntries(file, c) {
		return resolved(reg, ClassUnion, "union", High,
			"both sides only added entries to the same import or dependency list; keeping every entry once", unionInsertions(c))
	}

	// 3. neighbouring edits that do not overlap
	if disjoint(c.OursHunks, c.TheirsHunks) {
		return resolved(reg, ClassAdjacent, "both", Medium,
			"the two sides changed neighbouring lines without overlapping them; the merge engine treats touching edits as a conflict, applying both is mechanical but the lines may belong together",
			applyBoth(c))
	}

	// 4. both added the same lines at the same spot
	if len(c.Base) == 0 {
		if common := sharedSubstantialLines(c.Ours, c.Theirs); len(common) > 0 {
			return resolved(reg, ClassSameLines, "union", Low,
				fmt.Sprintf("both sides added the same line(s) (%s) at the same place; keeping them once and both sides' other additions in stream-then-proposal order", quoteFirst(common)),
				unionLines(c.Ours, c.Theirs))
		}
	}

	reg.Class, reg.Resolution = ClassNeedsPerson, "none"
	reg.Why = "both sides changed the same lines differently; choosing between them is a decision for a person"
	return reg, nil
}

func resolved(reg Region, class, resolution, conf, why string, lines []string) (Region, []string) {
	reg.Class, reg.Resolution, reg.Confidence, reg.Why = class, resolution, conf, why
	reg.Proposed = clip(textdiff.Join(lines))
	return reg, lines
}

func quoteFirst(lines []string) string {
	s := strings.TrimSpace(lines[0])
	if len(s) > 40 {
		s = s[:40] + "…"
	}
	if len(lines) > 1 {
		return fmt.Sprintf("%q and %d more", s, len(lines)-1)
	}
	return fmt.Sprintf("%q", s)
}

// wsEqual compares two texts ignoring whitespace differences inside lines and
// blank lines.
func wsEqual(a, b string) bool {
	norm := func(s string) []string {
		var out []string
		for _, l := range strings.Split(s, "\n") {
			if f := squashSpaces(l); f != "" {
				out = append(out, f)
			}
		}
		return out
	}
	x, y := norm(a), norm(b)
	if len(x) != len(y) {
		return false
	}
	for i := range x {
		if x[i] != y[i] {
			return false
		}
	}
	return true
}

// squashSpaces collapses runs of whitespace and drops whitespace next to
// punctuation, so "f( a )" and "f(a)" compare equal while "a b" and "ab" do not.
func squashSpaces(line string) string {
	collapsed := strings.Join(strings.Fields(line), " ")
	isWord := func(r rune) bool {
		return r == '_' || r >= '0' && r <= '9' || r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r > 127
	}
	rs := []rune(collapsed)
	var b strings.Builder
	for i, r := range rs {
		if r == ' ' && !(i > 0 && i+1 < len(rs) && isWord(rs[i-1]) && isWord(rs[i+1])) {
			continue
		}
		b.WriteRune(r)
	}
	return b.String()
}

func onlyInsertions(hs []textdiff.Hunk) bool {
	if len(hs) == 0 {
		return false
	}
	for _, h := range hs {
		if h.AStart != h.AEnd {
			return false
		}
	}
	return true
}

// insertionsAreListEntries reports whether every line either side inserted is
// an entry of an import or dependency list for this kind of file.
func insertionsAreListEntries(file string, c textdiff.Chunk) bool {
	added := append(extraLines(c.Base, c.Ours), extraLines(c.Base, c.Theirs)...)
	if len(added) == 0 {
		return false
	}
	for _, l := range added {
		if !isListLine(file, l) {
			return false
		}
	}
	return true
}

// extraLines returns the lines of side that are not part of base (multiset difference).
func extraLines(base, side []string) []string {
	have := map[string]int{}
	for _, l := range base {
		have[l]++
	}
	var out []string
	for _, l := range side {
		if have[l] > 0 {
			have[l]--
			continue
		}
		out = append(out, l)
	}
	return out
}

// unionInsertions builds base + ours' insertions + theirs' insertions (once),
// position by position. Sorted lists stay sorted.
func unionInsertions(c textdiff.Chunk) []string {
	type ins struct{ ours, theirs []string }
	at := map[int]*ins{}
	get := func(p int) *ins {
		if at[p] == nil {
			at[p] = &ins{}
		}
		return at[p]
	}
	for _, h := range c.OursHunks {
		g := get(h.AStart - c.BaseStart)
		g.ours = append(g.ours, sideHunkLines(c.Ours, c.OursHunks, c, h)...)
	}
	for _, h := range c.TheirsHunks {
		g := get(h.AStart - c.BaseStart)
		g.theirs = append(g.theirs, sideHunkLines(c.Theirs, c.TheirsHunks, c, h)...)
	}
	var out []string
	for pos := 0; pos <= len(c.Base); pos++ {
		if g := at[pos]; g != nil {
			out = append(out, unionLines(g.ours, g.theirs)...)
		}
		if pos < len(c.Base) {
			out = append(out, c.Base[pos])
		}
	}
	if sortedLines(c.Base) && sortedLines(c.Ours) && sortedLines(c.Theirs) {
		return sortUnique(out)
	}
	return out
}

func unionLines(a, b []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, l := range append(append([]string{}, a...), b...) {
		k := strings.TrimSpace(l)
		if k == "" {
			out = append(out, l)
			continue
		}
		if !seen[k] {
			seen[k] = true
			out = append(out, l)
		}
	}
	return out
}

func sortedLines(ls []string) bool {
	keys := make([]string, 0, len(ls))
	for _, l := range ls {
		if k := strings.TrimSpace(l); k != "" {
			keys = append(keys, k)
		}
	}
	return sort.StringsAreSorted(keys)
}

func sortUnique(ls []string) []string {
	seen := map[string]bool{}
	var out []string
	for _, l := range ls {
		if k := strings.TrimSpace(l); k != "" && !seen[k] {
			seen[k] = true
			out = append(out, l)
		}
	}
	sort.SliceStable(out, func(i, j int) bool { return strings.TrimSpace(out[i]) < strings.TrimSpace(out[j]) })
	return out
}

// disjoint: no hunk of ours overlaps a hunk of theirs, so both can be applied.
func disjoint(os, ts []textdiff.Hunk) bool {
	if len(os) == 0 || len(ts) == 0 {
		return false
	}
	for _, o := range os {
		for _, t := range ts {
			if hunksOverlap(o, t) {
				return false
			}
		}
	}
	return true
}

func hunksOverlap(a, b textdiff.Hunk) bool {
	aIns, bIns := a.AStart == a.AEnd, b.AStart == b.AEnd
	switch {
	case aIns && bIns:
		return a.AStart == b.AStart
	case aIns:
		return b.AStart < a.AStart && a.AStart < b.AEnd
	case bIns:
		return a.AStart < b.AStart && b.AStart < a.AEnd
	}
	return max(a.AStart, b.AStart) < min(a.AEnd, b.AEnd)
}

// applyBoth rebuilds the chunk with the hunks of both sides applied to the base.
func applyBoth(c textdiff.Chunk) []string {
	type h struct {
		textdiff.Hunk
		lines []string
		order int
	}
	var all []h
	for _, x := range c.OursHunks {
		all = append(all, h{x, sideHunkLines(c.Ours, c.OursHunks, c, x), 0})
	}
	for _, x := range c.TheirsHunks {
		all = append(all, h{x, sideHunkLines(c.Theirs, c.TheirsHunks, c, x), 1})
	}
	sort.SliceStable(all, func(i, j int) bool {
		if all[i].AStart != all[j].AStart {
			return all[i].AStart < all[j].AStart
		}
		// at the same spot an insertion goes before the replacement it touches
		ii, jj := all[i].AStart == all[i].AEnd, all[j].AStart == all[j].AEnd
		if ii != jj {
			return ii
		}
		return all[i].order < all[j].order
	})
	var out []string
	pos := c.BaseStart
	for _, x := range all {
		out = append(out, c.Base[pos-c.BaseStart:x.AStart-c.BaseStart]...)
		out = append(out, x.lines...)
		if x.AEnd > pos {
			pos = x.AEnd
		}
	}
	return append(out, c.Base[pos-c.BaseStart:]...)
}

// sideHunkLines extracts the replacement text of one hunk from a side's chunk
// text, by replaying that side's hunks in order.
func sideHunkLines(sideText []string, hunks []textdiff.Hunk, c textdiff.Chunk, target textdiff.Hunk) []string {
	pos := c.BaseStart
	off := 0
	for _, x := range hunks {
		off += x.AStart - pos // unchanged base lines before this hunk
		n := x.BEnd - x.BStart
		if x == target {
			if off+n > len(sideText) {
				return nil
			}
			return sideText[off : off+n]
		}
		off += n
		pos = x.AEnd
	}
	return nil
}

// sharedSubstantialLines are lines present in both slices that carry content
// (not just braces or blank lines).
var trivialLine = regexp.MustCompile(`^[\s{}()\[\];,]*$`)

func sharedSubstantialLines(a, b []string) []string {
	in := map[string]bool{}
	for _, l := range a {
		in[strings.TrimSpace(l)] = true
	}
	var out []string
	for _, l := range b {
		k := strings.TrimSpace(l)
		if k != "" && !trivialLine.MatchString(k) && in[k] {
			out = append(out, l)
		}
	}
	return out
}

// ---- list-like files ---------------------------------------------------------------

var (
	reGoImport   = regexp.MustCompile(`^(import\s+(\w+\s+)?"[^"]+"|(\w+|\.|_)\s+"[^"]+"|"[^"]+")$`)
	rePyImport   = regexp.MustCompile(`^(import\s+[\w.]+(\s+as\s+\w+)?|from\s+[\w.]+\s+import\s+[\w*,\s().]+)$`)
	reJSImport   = regexp.MustCompile(`^(import\s+.+\s+from\s+['"][^'"]+['"];?|import\s+['"][^'"]+['"];?|(const|let|var)\s+[\w{},\s]+=\s*require\(['"][^'"]+['"]\);?)$`)
	reRustUse    = regexp.MustCompile(`^(pub\s+)?(use\s+[\w:{}*,\s]+;|mod\s+\w+;|extern\s+crate\s+\w+;)$`)
	reCInclude   = regexp.MustCompile(`^#\s*include\s+[<"][^>"]+[>"]$`)
	reRequire    = regexp.MustCompile(`^[A-Za-z0-9_.\-]+(\[[A-Za-z0-9_,\-]+\])?\s*((==|>=|<=|~=|!=|>|<)\s*[\w.*+!\-]+(\s*,\s*(==|>=|<=|~=|!=|>|<)\s*[\w.*+!\-]+)*)?(\s*;.*)?$`)
	reGoMod      = regexp.MustCompile(`^(require\s+)?[A-Za-z0-9_.\-/~]+\s+v[0-9][\w.+\-]*(\s*//.*)?$`)
	reCargoDep   = regexp.MustCompile(`^[A-Za-z0-9_\-]+\s*=\s*("[^"]*"|\{[^}]*\})$`)
	reIgnoreLine = regexp.MustCompile(`^[!]?[^\s#][^\s]*$`)
	reGem        = regexp.MustCompile(`^gem\s+['"][^'"]+['"](\s*,\s*.+)?$`)
)

type listKind int

const (
	notList listKind = iota
	goSrc
	pySrc
	jsSrc
	rustSrc
	cSrc
	requirementsTxt
	goMod
	cargoToml
	ignoreFile
	gemfile
)

func kindOfFile(p string) listKind {
	base := path.Base(p)
	switch {
	case base == "go.mod":
		return goMod
	case base == "Cargo.toml":
		return cargoToml
	case base == "Gemfile":
		return gemfile
	case base == ".gitignore" || base == ".dockerignore":
		return ignoreFile
	case strings.HasPrefix(base, "requirements") && strings.HasSuffix(base, ".txt"):
		return requirementsTxt
	}
	switch path.Ext(base) {
	case ".go":
		return goSrc
	case ".py":
		return pySrc
	case ".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs":
		return jsSrc
	case ".rs":
		return rustSrc
	case ".c", ".h", ".cc", ".cpp", ".hpp", ".cxx":
		return cSrc
	}
	return notList
}

// isListLine says whether a line is an entry of an import or dependency list
// for the kind of file it sits in. Unknown file kinds never qualify.
func isListLine(file, line string) bool {
	l := strings.TrimSpace(line)
	if l == "" {
		return false
	}
	switch kindOfFile(file) {
	case goSrc:
		return reGoImport.MatchString(l)
	case pySrc:
		return rePyImport.MatchString(l)
	case jsSrc:
		return reJSImport.MatchString(l)
	case rustSrc:
		return reRustUse.MatchString(l)
	case cSrc:
		return reCInclude.MatchString(l)
	case requirementsTxt:
		return reRequire.MatchString(l)
	case goMod:
		return reGoMod.MatchString(l)
	case cargoToml:
		return reCargoDep.MatchString(l)
	case ignoreFile:
		return reIgnoreLine.MatchString(l)
	case gemfile:
		return reGem.MatchString(l)
	}
	return false
}

// allListLike: a whole list file (comments and blank lines allowed).
func allListLike(file, content string) bool {
	if kindOfFile(file) == notList {
		return false
	}
	switch kindOfFile(file) {
	case goSrc, pySrc, jsSrc, rustSrc, cSrc:
		return false // a source file is never "just a list"
	}
	entries := 0
	for _, l := range strings.Split(content, "\n") {
		t := strings.TrimSpace(l)
		if t == "" || strings.HasPrefix(t, "#") {
			continue
		}
		if !isListLine(file, t) {
			return false
		}
		entries++
	}
	return entries > 0
}
