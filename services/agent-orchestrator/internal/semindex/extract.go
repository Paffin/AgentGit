// Package semindex is a lightweight, incremental symbol index of a stream:
// the definitions in each source file (Go, Rust, Python, TypeScript and
// JavaScript), a reference-count heuristic, and a "semantic overlap" view that
// tells which open proposals change the same symbols.
//
// It is advisory. The extractors are careful line-based scanners, not parsers:
// they find the declarations that gofmt/rustfmt/black/prettier-style code
// puts at predictable places and ignore what they cannot be sure about.
// Nothing here influences what may land; the merge engine, validation and
// policy decide that.
package semindex

import (
	"path"
	"regexp"
	"sort"
	"strings"
	"unicode"
)

// Language names.
const (
	LangGo     = "go"
	LangRust   = "rust"
	LangPython = "python"
	LangTS     = "typescript"
	LangJS     = "javascript"
)

// LanguageOf maps a path to a supported language ("" = not indexed).
func LanguageOf(p string) string {
	switch strings.ToLower(path.Ext(p)) {
	case ".go":
		return LangGo
	case ".rs":
		return LangRust
	case ".py":
		return LangPython
	case ".ts", ".tsx":
		return LangTS
	case ".js", ".jsx", ".mjs", ".cjs":
		return LangJS
	}
	return ""
}

// Limits on what one file may contribute.
const (
	maxDefsPerFile   = 500
	maxIdentsPerFile = 512
	maxSigLen        = 160
)

// Def is one definition.
type Def struct {
	Name      string `json:"name"`
	Container string `json:"container,omitempty"` // receiver, impl type or class
	Kind      string `json:"kind"`                // func, method, type, struct, interface, enum, trait, class, const, var, mod, macro
	Line      int    `json:"line"`                // 1-based
	EndLine   int    `json:"end_line"`
	Exported  bool   `json:"exported"`
	Sig       string `json:"sig,omitempty"`
}

// Qualified is Container.Name, or just Name.
func (d Def) Qualified() string {
	if d.Container == "" {
		return d.Name
	}
	return d.Container + "." + d.Name
}

// Symbols is what one file contributes to the index.
type Symbols struct {
	Lang   string            `json:"lang"`
	Defs   []Def             `json:"defs"`
	Idents map[string]uint16 `json:"idents,omitempty"`
	Lines  int               `json:"lines"`
	// DroppedDefs and DroppedIdents count what the per-file bounds cut (maxDefsPerFile, maxIdentsPerFile): the
	// index says how incomplete it is instead of being silently short.
	DroppedDefs   int `json:"dropped_defs,omitempty"`
	DroppedIdents int `json:"dropped_idents,omitempty"`
}

// Extract scans one file. known is the set of names currently defined in the
// index; identifiers in it are kept first when the per-file identifier budget
// is tight, so reference counts favour symbols that exist.
func Extract(lang string, content []byte, known map[string]bool) Symbols {
	text := string(content)
	lines := strings.Split(text, "\n")
	var defs []Def
	switch lang {
	case LangGo:
		defs = scanGo(lines)
	case LangRust:
		defs = scanRust(lines)
	case LangPython:
		defs = scanPython(lines)
	case LangTS, LangJS:
		defs = scanJS(lines)
	}
	droppedDefs := 0
	if len(defs) > maxDefsPerFile {
		droppedDefs = len(defs) - maxDefsPerFile
		defs = defs[:maxDefsPerFile]
	}
	finishSpans(defs, len(lines))
	idents, droppedIdents := identCounts(lang, text, known)
	return Symbols{Lang: lang, Defs: defs, Idents: idents, Lines: len(lines), DroppedDefs: droppedDefs, DroppedIdents: droppedIdents}
}

// finishSpans sets EndLine: a definition runs until the next one starts.
func finishSpans(defs []Def, nLines int) {
	for i := range defs {
		defs[i].EndLine = nLines
		if i+1 < len(defs) {
			defs[i].EndLine = max(defs[i].Line, defs[i+1].Line-1)
		}
	}
}

// clipSig reduces a declaration line to its signature: everything before the
// body's opening brace (a brace inside parentheses belongs to a type literal),
// so that editing a one-line function's body is not mistaken for changing its
// signature.
func clipSig(s string) string {
	s = strings.TrimSpace(s)
	depth := 0
scan:
	for i, r := range s {
		switch r {
		case '(', '[':
			depth++
		case ')', ']':
			depth--
		case '{':
			if depth <= 0 {
				s = s[:i]
				break scan
			}
		}
	}
	s = strings.TrimSpace(s)
	if len(s) > maxSigLen {
		s = s[:maxSigLen] + "…"
	}
	return s
}

func isUpperFirst(s string) bool {
	for _, r := range s {
		return unicode.IsUpper(r)
	}
	return false
}

// ---- Go ------------------------------------------------------------------------------------

var (
	goFunc  = regexp.MustCompile(`^func\s+(?:\(\s*(?:\w+\s+)?\*?\s*([A-Za-z_]\w*)(?:\[[^\]]*\])?\s*\)\s*)?([A-Za-z_]\w*)\s*[\[(]`)
	goType  = regexp.MustCompile(`^type\s+([A-Za-z_]\w*)(?:\[[^\]]*\])?\s+(struct|interface|=|\S+)`)
	goVar   = regexp.MustCompile(`^(const|var)\s+([A-Za-z_]\w*)\b`)
	goGroup = regexp.MustCompile(`^(const|var|type)\s*\($`)
	goEntry = regexp.MustCompile(`^\t([A-Za-z_]\w*)\b(?:\s+(struct|interface)\b)?`)
)

func scanGo(lines []string) []Def {
	var defs []Def
	group := ""
	inBlock := false
	for i, raw := range lines {
		line := strings.TrimRight(raw, "\r")
		if inBlock {
			if strings.Contains(line, "*/") {
				inBlock = false
			}
			continue
		}
		if strings.HasPrefix(line, "/*") && !strings.Contains(line, "*/") {
			inBlock = true
			continue
		}
		if group != "" {
			if strings.HasPrefix(line, ")") {
				group = ""
				continue
			}
			if m := goEntry.FindStringSubmatch(line); m != nil && m[1] != "_" {
				kind := group
				if group == "type" {
					kind = "type"
					if m[2] != "" {
						kind = m[2]
					}
				}
				defs = append(defs, Def{Name: m[1], Kind: kind, Line: i + 1, Exported: isUpperFirst(m[1]), Sig: clipSig(line)})
			}
			continue
		}
		if m := goGroup.FindStringSubmatch(line); m != nil {
			group = m[1]
			continue
		}
		if m := goFunc.FindStringSubmatch(line); m != nil {
			kind := "func"
			if m[1] != "" {
				kind = "method"
			}
			exported := isUpperFirst(m[2]) && (m[1] == "" || isUpperFirst(m[1]))
			defs = append(defs, Def{Name: m[2], Container: m[1], Kind: kind, Line: i + 1, Exported: exported, Sig: clipSig(line)})
			continue
		}
		if m := goType.FindStringSubmatch(line); m != nil {
			kind := "type"
			if m[2] == "struct" || m[2] == "interface" {
				kind = m[2]
			}
			defs = append(defs, Def{Name: m[1], Kind: kind, Line: i + 1, Exported: isUpperFirst(m[1]), Sig: clipSig(line)})
			continue
		}
		if m := goVar.FindStringSubmatch(line); m != nil && m[2] != "_" {
			defs = append(defs, Def{Name: m[2], Kind: m[1], Line: i + 1, Exported: isUpperFirst(m[2]), Sig: clipSig(line)})
		}
	}
	return defs
}

// ---- Rust ------------------------------------------------------------------------------------

var (
	rsFn    = regexp.MustCompile(`^(\s*)(pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+([A-Za-z_]\w*)`)
	rsItem  = regexp.MustCompile(`^(\s*)(pub(?:\([^)]*\))?\s+)?(struct|enum|trait|union|type|mod|const|static)\s+(?:mut\s+)?([A-Za-z_]\w*)`)
	rsMacro = regexp.MustCompile(`^(\s*)macro_rules!\s+([A-Za-z_]\w*)`)
	rsImpl  = regexp.MustCompile(`^(\s*)(?:unsafe\s+)?impl\b(?:\s*<[^{]*?>)?\s+(?:[^{]*?\s+for\s+)?([A-Za-z_][\w:]*)`)
)

func indentOf(s string) int { return len(s) - len(strings.TrimLeft(s, " \t")) }

func isPlainPub(vis string) bool { return strings.TrimSpace(vis) == "pub" }

func scanRust(lines []string) []Def {
	var defs []Def
	inBlock := false
	implIndent, implType := -1, ""
	for i, raw := range lines {
		line := strings.TrimRight(raw, "\r")
		trimmed := strings.TrimSpace(line)
		if inBlock {
			if strings.Contains(line, "*/") {
				inBlock = false
			}
			continue
		}
		if strings.HasPrefix(trimmed, "/*") && !strings.Contains(trimmed, "*/") {
			inBlock = true
			continue
		}
		if trimmed == "" || strings.HasPrefix(trimmed, "//") {
			continue
		}
		if implIndent >= 0 && indentOf(line) <= implIndent && !strings.HasPrefix(trimmed, "impl") {
			implIndent, implType = -1, ""
		}
		if m := rsImpl.FindStringSubmatch(line); m != nil {
			implIndent, implType = indentOf(line), strings.TrimPrefix(m[2], "crate::")
			continue
		}
		if m := rsMacro.FindStringSubmatch(line); m != nil {
			defs = append(defs, Def{Name: m[2], Kind: "macro", Line: i + 1, Exported: true, Sig: clipSig(line)})
			continue
		}
		if m := rsFn.FindStringSubmatch(line); m != nil {
			d := Def{Name: m[3], Kind: "func", Line: i + 1, Exported: isPlainPub(m[2]), Sig: clipSig(line)}
			if implIndent >= 0 && indentOf(line) > implIndent {
				d.Kind, d.Container = "method", implType
			}
			defs = append(defs, d)
			continue
		}
		if m := rsItem.FindStringSubmatch(line); m != nil {
			// "type"/"const" inside impl or trait blocks are associated items; keep them with their container
			d := Def{Name: m[4], Kind: m[3], Line: i + 1, Exported: isPlainPub(m[2]), Sig: clipSig(line)}
			if implIndent >= 0 && indentOf(line) > implIndent {
				d.Container = implType
			}
			defs = append(defs, d)
		}
	}
	return defs
}

// ---- Python ------------------------------------------------------------------------------------

var (
	pyDef   = regexp.MustCompile(`^(\s*)(?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(`)
	pyClass = regexp.MustCompile(`^(\s*)class\s+([A-Za-z_]\w*)`)
	pyConst = regexp.MustCompile(`^(_?[A-Z][A-Z0-9_]*)\s*(?::[^=]+)?=[^=]`)
)

func scanPython(lines []string) []Def {
	var defs []Def
	type frame struct {
		indent, body int // body: indentation of the class body, -1 until its first line is seen
		name         string
	}
	var classes []frame
	inDoc := ""
	for i, raw := range lines {
		line := strings.TrimRight(raw, "\r")
		if inDoc != "" {
			if strings.Contains(line, inDoc) {
				inDoc = ""
			}
			continue
		}
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "#") {
			continue
		}
		for _, q := range []string{`"""`, `'''`} {
			if strings.Count(line, q)%2 == 1 {
				inDoc = q
			}
		}
		ind := indentOf(line)
		for len(classes) > 0 && ind <= classes[len(classes)-1].indent {
			classes = classes[:len(classes)-1]
		}
		if len(classes) > 0 && classes[len(classes)-1].body < 0 {
			classes[len(classes)-1].body = ind
		}
		if inDoc != "" {
			continue
		}
		if m := pyClass.FindStringSubmatch(line); m != nil {
			container := ""
			if len(classes) > 0 {
				container = classes[len(classes)-1].name
			}
			defs = append(defs, Def{Name: m[2], Container: container, Kind: "class", Line: i + 1, Exported: !strings.HasPrefix(m[2], "_"), Sig: clipSig(line)})
			classes = append(classes, frame{indent: len(m[1]), body: -1, name: m[2]})
			continue
		}
		if m := pyDef.FindStringSubmatch(line); m != nil {
			switch {
			case len(classes) > 0:
				if top := classes[len(classes)-1]; len(m[1]) == top.body { // directly in the class body; deeper ones are nested functions
					defs = append(defs, Def{Name: m[2], Container: top.name, Kind: "method", Line: i + 1, Exported: !strings.HasPrefix(m[2], "_"), Sig: clipSig(line)})
				}
			case len(m[1]) == 0:
				defs = append(defs, Def{Name: m[2], Kind: "func", Line: i + 1, Exported: !strings.HasPrefix(m[2], "_"), Sig: clipSig(line)})
			}
			continue
		}
		if len(classes) == 0 {
			if m := pyConst.FindStringSubmatch(line); m != nil {
				defs = append(defs, Def{Name: m[1], Kind: "const", Line: i + 1, Exported: !strings.HasPrefix(m[1], "_"), Sig: clipSig(line)})
			}
		}
	}
	return defs
}

// ---- TypeScript / JavaScript ----------------------------------------------------------------------

var (
	jsFunc   = regexp.MustCompile(`^(export\s+(?:default\s+)?)?(?:declare\s+)?(?:async\s+)?function\s*\*?\s*([A-Za-z_$][\w$]*)`)
	jsClass  = regexp.MustCompile(`^(export\s+(?:default\s+)?)?(?:declare\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)`)
	jsIface  = regexp.MustCompile(`^(export\s+)?(?:declare\s+)?(interface|type|enum|namespace)\s+([A-Za-z_$][\w$]*)`)
	jsConst  = regexp.MustCompile(`^(export\s+)?(?:declare\s+)?(const|let|var)\s+([A-Za-z_$][\w$]*)\s*(?::[^=]+)?=`)
	jsMethod = regexp.MustCompile(`^\s+(?:(?:public|private|protected|static|readonly|override|abstract|async|get|set)\s+)*(#?[A-Za-z_$][\w$]*)\s*(?:<[^>]*>)?\s*\([^)]*\)\s*(?::\s*[^{;]+)?\s*\{\s*\}?\s*$`)
)

var jsNotMethod = map[string]bool{"if": true, "for": true, "while": true, "switch": true, "catch": true, "return": true, "function": true, "with": true, "else": true, "do": true, "try": true}

func scanJS(lines []string) []Def {
	var defs []Def
	inBlock := false
	className, classIndent, classExported := "", -1, false
	for i, raw := range lines {
		line := strings.TrimRight(raw, "\r")
		trimmed := strings.TrimSpace(line)
		if inBlock {
			if strings.Contains(line, "*/") {
				inBlock = false
			}
			continue
		}
		if strings.HasPrefix(trimmed, "/*") && !strings.Contains(trimmed, "*/") {
			inBlock = true
			continue
		}
		if trimmed == "" || strings.HasPrefix(trimmed, "//") {
			continue
		}
		if className != "" && indentOf(line) <= classIndent && !strings.HasPrefix(trimmed, "class") && trimmed != "" {
			className, classIndent, classExported = "", -1, false
		}
		if m := jsClass.FindStringSubmatch(line); m != nil {
			defs = append(defs, Def{Name: m[2], Kind: "class", Line: i + 1, Exported: m[1] != "", Sig: clipSig(line)})
			className, classIndent, classExported = m[2], indentOf(line), m[1] != ""
			continue
		}
		if m := jsFunc.FindStringSubmatch(line); m != nil {
			defs = append(defs, Def{Name: m[2], Kind: "func", Line: i + 1, Exported: m[1] != "", Sig: clipSig(line)})
			continue
		}
		if m := jsIface.FindStringSubmatch(line); m != nil {
			defs = append(defs, Def{Name: m[3], Kind: m[2], Line: i + 1, Exported: m[1] != "", Sig: clipSig(line)})
			continue
		}
		if m := jsConst.FindStringSubmatch(line); m != nil {
			defs = append(defs, Def{Name: m[3], Kind: m[2], Line: i + 1, Exported: m[1] != "", Sig: clipSig(line)})
			continue
		}
		if className != "" && indentOf(line) > classIndent {
			if m := jsMethod.FindStringSubmatch(line); m != nil && !jsNotMethod[m[1]] {
				private := strings.HasPrefix(m[1], "#") || strings.Contains(line, "private ") || strings.Contains(line, "protected ")
				defs = append(defs, Def{Name: strings.TrimPrefix(m[1], "#"), Container: className, Kind: "method", Line: i + 1, Exported: classExported && !private, Sig: clipSig(line)})
			}
		}
	}
	return defs
}

// ---- identifiers -----------------------------------------------------------------------------------

var identRE = regexp.MustCompile(`[A-Za-z_$][A-Za-z0-9_$]{2,}`)

var keywords = map[string]bool{}

func init() {
	for _, w := range strings.Fields(`func package import return range switch select struct interface type const var map chan defer go else for if break continue case default fallthrough goto nil true false iota string int bool byte rune error any
		fn let mut pub use mod impl trait enum match loop while self Self super crate where async await move dyn ref static unsafe extern as in Some None Ok Err Box Vec Option Result String
		def class from lambda pass with yield not and or is None True False del global nonlocal raise try except finally assert elif print
		function const let var new this typeof instanceof void null undefined export default extends implements static public private protected readonly abstract declare number boolean interface throw catch finally super of get set from require module exports`) {
		keywords[w] = true
	}
}

// identCounts counts identifier occurrences, keeping at most maxIdentsPerFile:
// names already defined somewhere come first, then the most frequent.
func identCounts(lang, text string, known map[string]bool) (map[string]uint16, int) {
	counts := map[string]int{}
	for _, m := range identRE.FindAllString(text, -1) {
		if !keywords[m] {
			counts[m]++
		}
	}
	if len(counts) <= maxIdentsPerFile {
		return clamp(counts), 0
	}
	type kv struct {
		k string
		v int
		d bool
	}
	all := make([]kv, 0, len(counts))
	for k, v := range counts {
		all = append(all, kv{k, v, known[k]})
	}
	sort.Slice(all, func(i, j int) bool {
		if all[i].d != all[j].d {
			return all[i].d
		}
		if all[i].v != all[j].v {
			return all[i].v > all[j].v
		}
		return all[i].k < all[j].k
	})
	kept := map[string]int{}
	for _, e := range all[:maxIdentsPerFile] {
		kept[e.k] = e.v
	}
	return clamp(kept), len(counts) - len(kept)
}

func clamp(m map[string]int) map[string]uint16 {
	out := make(map[string]uint16, len(m))
	for k, v := range m {
		if v > 65535 {
			v = 65535
		}
		out[k] = uint16(v)
	}
	return out
}
