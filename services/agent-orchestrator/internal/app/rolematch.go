package app

import (
	"fmt"
	"regexp"
	"strings"
)

// hasRole reports whether roles ("pattern:role") grant one of want on repo.
func hasRole(roles []string, repo string, want []string) bool {
	has, _ := roleCheck(roles, repo, want)
	return has
}

// roleCheck reports whether roles ("pattern:role") grant one of want on repo. unsure lists the patterns of a
// wanted role that could not be translated to a local matcher; they are neither matches nor proof of absence.
func roleCheck(roles []string, repo string, want []string) (has bool, unsure []string) {
	for _, r := range roles {
		i := strings.LastIndex(r, ":")
		if i < 0 {
			continue
		}
		pattern, role := r[:i], r[i+1:]
		ok := false
		for _, w := range want {
			if role == w {
				ok = true
			}
		}
		if !ok {
			continue
		}
		re, err := globToRegexp(pattern)
		if err != nil {
			unsure = append(unsure, fmt.Sprintf("%q", pattern))
			continue
		}
		if re.MatchString(repo) {
			return true, nil
		}
	}
	return false, unsure
}

// globToRegexp translates a role-binding pattern to an anchored regular expression with the semantics the server
// gives it (crates/ag-server/src/authz.rs uses the globset crate with its defaults): `*` matches any run of
// characters including `/`, `?` any one character, `[abc]`, `[a-c]` and `[!abc]` a class, `{a,b}` alternatives,
// and a backslash escapes. A pattern this function cannot translate is an error, never a guess.
func globToRegexp(pattern string) (*regexp.Regexp, error) {
	var b strings.Builder
	b.WriteString("(?s)^")
	rs := []rune(pattern)
	depth := 0
	for i := 0; i < len(rs); i++ {
		switch c := rs[i]; c {
		case '*':
			for i+1 < len(rs) && rs[i+1] == '*' {
				i++
			}
			b.WriteString(".*")
		case '?':
			b.WriteString(".")
		case '\\':
			if i+1 >= len(rs) {
				return nil, fmt.Errorf("dangling escape")
			}
			i++
			b.WriteString(regexp.QuoteMeta(string(rs[i])))
		case '[':
			j := i + 1
			neg := false
			if j < len(rs) && (rs[j] == '!' || rs[j] == '^') {
				neg = true
				j++
			}
			start := j
			if j < len(rs) && rs[j] == ']' { // a leading ] is a member
				j++
			}
			for j < len(rs) && rs[j] != ']' {
				j++
			}
			if j >= len(rs) {
				return nil, fmt.Errorf("unclosed character class")
			}
			members := rs[start:j]
			if len(members) == 0 {
				return nil, fmt.Errorf("empty character class")
			}
			b.WriteString("[")
			if neg {
				b.WriteString("^")
			}
			for _, m := range members {
				if m == '\\' || m == '[' || m == ']' || m == '^' {
					b.WriteString(`\`)
				}
				b.WriteRune(m)
			}
			b.WriteString("]")
			i = j
		case '{':
			depth++
			b.WriteString("(?:")
		case '}':
			if depth == 0 {
				return nil, fmt.Errorf("unbalanced }")
			}
			depth--
			b.WriteString(")")
		case ',':
			if depth > 0 {
				b.WriteString("|")
			} else {
				b.WriteString(",")
			}
		default:
			b.WriteString(regexp.QuoteMeta(string(c)))
		}
	}
	if depth != 0 {
		return nil, fmt.Errorf("unclosed {")
	}
	b.WriteString("$")
	return regexp.Compile(b.String())
}
