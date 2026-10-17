// Package glob implements the path globs AgentGit uses for task scopes,
// leases and protected paths.
//
// Syntax (the same family the server's matcher accepts):
//
//	"x*y"    any run of characters inside one path component
//	"x?y"    one character inside a component
//	"a/**/b" any number of whole components (only as a full component)
//	"[abc]"  a character class; [!abc] or [^abc] negates; ranges a-z work
//	"{a,b}"  alternatives (may nest; at most 64 expansions)
//	"\x"     the literal character x
//
// "src/**" matches everything below src; "**/*.rs" matches Rust files at any
// depth, including the top level.
package glob

import (
	"errors"
	"fmt"
	"regexp"
	"strings"
)

const maxExpansions = 64

// Glob is a compiled pattern.
type Glob struct {
	pattern string
	res     []*regexp.Regexp
}

// Compile parses pattern.
func Compile(pattern string) (*Glob, error) {
	if pattern == "" {
		return nil, errors.New("empty glob")
	}
	alts, err := expandBraces(pattern)
	if err != nil {
		return nil, err
	}
	g := &Glob{pattern: pattern}
	for _, a := range alts {
		src, err := translate(a)
		if err != nil {
			return nil, fmt.Errorf("glob %q: %w", pattern, err)
		}
		re, err := regexp.Compile(src)
		if err != nil {
			return nil, fmt.Errorf("glob %q: %w", pattern, err)
		}
		g.res = append(g.res, re)
	}
	return g, nil
}

// MustCompile is Compile for constants; it panics on error.
func MustCompile(pattern string) *Glob {
	g, err := Compile(pattern)
	if err != nil {
		panic(err)
	}
	return g
}

// String returns the source pattern.
func (g *Glob) String() string { return g.pattern }

// Match reports whether the slash-separated path matches.
func (g *Glob) Match(path string) bool {
	path = strings.TrimPrefix(path, "./")
	for _, re := range g.res {
		if re.MatchString(path) {
			return true
		}
	}
	return false
}

// Set is a list of compiled globs.
type Set []*Glob

// CompileAll compiles every pattern, reporting the first that is invalid.
func CompileAll(patterns []string) (Set, error) {
	out := make(Set, 0, len(patterns))
	for _, p := range patterns {
		g, err := Compile(p)
		if err != nil {
			return nil, err
		}
		out = append(out, g)
	}
	return out, nil
}

// Match reports whether any glob matches.
func (s Set) Match(path string) bool {
	for _, g := range s {
		if g.Match(path) {
			return true
		}
	}
	return false
}

// First returns the first pattern that matches, or "".
func (s Set) First(path string) string {
	for _, g := range s {
		if g.Match(path) {
			return g.pattern
		}
	}
	return ""
}

// LiteralPrefix is the text of pattern before its first wildcard.
func LiteralPrefix(pattern string) string {
	if i := strings.IndexAny(pattern, "*?[{"); i >= 0 {
		return pattern[:i]
	}
	return pattern
}

// MayOverlap is a conservative test: it is true unless the literal prefixes of
// the two patterns prove that no path can match both. It deliberately mirrors
// the server's lease-overlap rule so scheduling decisions and server-side lease
// conflicts agree. It can report an overlap that is not real, never miss one.
func MayOverlap(a, b string) bool {
	pa, pb := LiteralPrefix(a), LiteralPrefix(b)
	return strings.HasPrefix(pa, pb) || strings.HasPrefix(pb, pa)
}

// AnyOverlap reports whether any pattern of as may overlap any pattern of bs.
func AnyOverlap(as, bs []string) bool {
	for _, a := range as {
		for _, b := range bs {
			if MayOverlap(a, b) {
				return true
			}
		}
	}
	return false
}

// expandBraces expands {a,b} alternatives (honouring \ escapes and nesting).
func expandBraces(p string) ([]string, error) {
	out, err := expand(p)
	if err != nil {
		return nil, err
	}
	if len(out) > maxExpansions {
		return nil, fmt.Errorf("glob %q expands to more than %d alternatives", p, maxExpansions)
	}
	return out, nil
}

func expand(p string) ([]string, error) {
	open := -1
	depth := 0
	for i := 0; i < len(p); i++ {
		switch p[i] {
		case '\\':
			i++
		case '[':
			// skip a character class, braces inside are literal
			j := i + 1
			if j < len(p) && (p[j] == '!' || p[j] == '^') {
				j++
			}
			if j < len(p) && p[j] == ']' {
				j++
			}
			for j < len(p) && p[j] != ']' {
				j++
			}
			if j < len(p) {
				i = j
			}
		case '{':
			if depth == 0 {
				open = i
			}
			depth++
		case '}':
			if depth == 0 {
				continue
			}
			depth--
			if depth == 0 {
				parts := splitTop(p[open+1 : i])
				var res []string
				for _, part := range parts {
					tails, err := expand(p[:open] + part + p[i+1:])
					if err != nil {
						return nil, err
					}
					res = append(res, tails...)
					if len(res) > maxExpansions {
						return nil, fmt.Errorf("glob %q expands to more than %d alternatives", p, maxExpansions)
					}
				}
				return res, nil
			}
		}
	}
	if depth != 0 {
		return nil, fmt.Errorf("unclosed '{' in glob %q", p)
	}
	return []string{p}, nil
}

func splitTop(s string) []string {
	var parts []string
	depth, start := 0, 0
	for i := 0; i < len(s); i++ {
		switch s[i] {
		case '\\':
			i++
		case '{':
			depth++
		case '}':
			depth--
		case ',':
			if depth == 0 {
				parts = append(parts, s[start:i])
				start = i + 1
			}
		}
	}
	return append(parts, s[start:])
}

// translate converts one brace-free pattern to an anchored regexp.
func translate(p string) (string, error) {
	var b strings.Builder
	b.WriteString(`^`)
	i := 0
	// component-aware handling of "**"
	for i < len(p) {
		c := p[i]
		switch c {
		case '\\':
			if i+1 >= len(p) {
				return "", errors.New("trailing backslash")
			}
			b.WriteString(regexp.QuoteMeta(string(p[i+1])))
			i += 2
		case '*':
			j := i
			for j < len(p) && p[j] == '*' {
				j++
			}
			stars := j - i
			atStart := i == 0 || p[i-1] == '/'
			atEnd := j == len(p) || p[j] == '/'
			if stars >= 2 && atStart && atEnd {
				switch {
				case i == 0 && j == len(p): // "**"
					b.WriteString(`.*`)
				case j == len(p): // "x/**": everything beneath x
					// the preceding "/" was already written; require something after it
					b.WriteString(`.+`)
				default: // "**/..." or ".../**/...": zero or more whole components
					b.WriteString(`(?:[^/]+/)*`)
					j++ // consume the following '/'
				}
			} else {
				b.WriteString(`[^/]*`)
			}
			i = j
		case '?':
			b.WriteString(`[^/]`)
			i++
		case '[':
			end, class, err := parseClass(p, i)
			if err != nil {
				return "", err
			}
			b.WriteString(class)
			i = end
		default:
			b.WriteString(regexp.QuoteMeta(string(c)))
			i++
		}
	}
	b.WriteString(`$`)
	return b.String(), nil
}

func parseClass(p string, i int) (int, string, error) {
	j := i + 1
	neg := false
	if j < len(p) && (p[j] == '!' || p[j] == '^') {
		neg = true
		j++
	}
	var body strings.Builder
	first := true
	for j < len(p) {
		if p[j] == ']' && !first {
			cls := "["
			if neg {
				cls += "^/" // a negated class must not match the separator
			}
			return j + 1, cls + body.String() + "]", nil
		}
		first = false
		if p[j] == '\\' && j+1 < len(p) {
			body.WriteString(regexp.QuoteMeta(string(p[j+1])))
			j += 2
			continue
		}
		if p[j] == '[' || p[j] == '^' && body.Len() > 0 {
			body.WriteString(`\`)
		}
		body.WriteByte(p[j])
		j++
	}
	return 0, "", errors.New("unclosed '['")
}
