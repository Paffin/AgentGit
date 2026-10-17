// Package review holds the deterministic reviewers: cheap, explainable checks
// on a proposal's diff that post "comment" reviews.
//
// The reviewers never approve and never request changes. Whether any
// automated reviewer's approval could ever count toward a policy's approval
// requirement is a decision of the server's policy (approvers lists identity
// patterns); this package does not try to influence it.
package review

import (
	"fmt"
	"math"
	"path"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/glob"
)

// Finding kinds (metric labels).
const (
	KindSecret    = "secret"
	KindProtected = "protected_path"
	KindOversized = "oversized"
	KindTests     = "missing_tests"
	KindGenerated = "generated_churn"
)

// Severities.
const (
	SevHigh = "high"
	SevWarn = "warn"
	SevInfo = "info"
)

// Thresholds that are not worth configuring.
const (
	// A change to sources with fewer added lines than this is not asked for tests.
	minSourceLines = 10
	// Generated or binary churn above this many lines (or any binary file) is mentioned.
	generatedLines = 200
	generatedFiles = 5
)

// Settings are the compiled review settings of a repository.
type Settings struct {
	MaxFiles  int
	MaxLines  int
	Protected glob.Set
	Generated glob.Set
	Sources   glob.Set
	Secrets   bool
	Tests     bool
	Allow     []*regexp.Regexp
}

// Finding is one thing a reviewer wants a person to look at.
type Finding struct {
	Kind     string   `json:"kind"`
	Severity string   `json:"severity"`
	Title    string   `json:"title"`
	Details  []string `json:"details,omitempty"`
}

// Summary describes the change that was looked at.
type Summary struct {
	Files     int  `json:"files"`
	Added     int  `json:"added"`
	Removed   int  `json:"removed"`
	Truncated bool `json:"truncated,omitempty"`
}

// Analyze runs every enabled check over the per-file diffs.
func Analyze(files []agwp.FileDiff, s Settings) ([]Finding, Summary) {
	sum := Summary{Files: len(files)}
	var counted, generated []agwp.FileDiff
	for _, f := range files {
		sum.Added += f.Additions
		sum.Removed += f.Deletions
		if s.Generated.Match(f.Path) || f.Binary {
			generated = append(generated, f)
		} else {
			counted = append(counted, f)
		}
	}
	var out []Finding
	if s.Secrets {
		out = append(out, secretFindings(files, s)...)
	}
	out = append(out, protectedFindings(files, s)...)
	out = append(out, sizeFindings(counted, s)...)
	if s.Tests {
		out = append(out, testFindings(counted, s)...)
	}
	out = append(out, generatedFindings(generated)...)
	rank := map[string]int{SevHigh: 0, SevWarn: 1, SevInfo: 2}
	sort.SliceStable(out, func(i, j int) bool { return rank[out[i].Severity] < rank[out[j].Severity] })
	return out, sum
}

// ---- size ------------------------------------------------------------------------

func sizeFindings(files []agwp.FileDiff, s Settings) []Finding {
	lines := 0
	for _, f := range files {
		lines += f.Additions + f.Deletions
	}
	if len(files) <= s.MaxFiles && lines <= s.MaxLines {
		return nil
	}
	return []Finding{{
		Kind: KindOversized, Severity: SevWarn,
		Title:   "Large change",
		Details: []string{fmt.Sprintf("%d files and %d changed lines (generated and binary files not counted); the configured limits are %d files and %d lines. Large changes are hard to review well; consider splitting this one.", len(files), lines, s.MaxFiles, s.MaxLines)},
	}}
}

// ---- protected paths ---------------------------------------------------------------

func protectedFindings(files []agwp.FileDiff, s Settings) []Finding {
	var hits []string
	for _, f := range files {
		if g := s.Protected.First(f.Path); g != "" {
			hits = append(hits, fmt.Sprintf("`%s` (%s, matches `%s`)", f.Path, f.Status, g))
		}
	}
	if len(hits) == 0 {
		return nil
	}
	sort.Strings(hits)
	return []Finding{{
		Kind: KindProtected, Severity: SevWarn,
		Title:   "Touches protected or sensitive paths",
		Details: append(clipList(hits, 10), "The stream policy's own protected-path rules decide what approvals these need; this note only points at them."),
	}}
}

func clipList(l []string, n int) []string {
	if len(l) <= n {
		return l
	}
	return append(append([]string{}, l[:n]...), fmt.Sprintf("… and %d more", len(l)-n))
}

// ---- generated and binary churn ----------------------------------------------------

func generatedFindings(files []agwp.FileDiff) []Finding {
	if len(files) == 0 {
		return nil
	}
	lines, binaries := 0, 0
	var names []string
	for _, f := range files {
		lines += f.Additions + f.Deletions
		if f.Binary {
			binaries++
		}
		names = append(names, fmt.Sprintf("`%s` (%s)", f.Path, f.Status))
	}
	if binaries == 0 && lines < generatedLines && len(files) < generatedFiles {
		return nil
	}
	sort.Strings(names)
	return []Finding{{
		Kind: KindGenerated, Severity: SevInfo,
		Title:   "Generated or binary files changed",
		Details: append(clipList(names, 8), fmt.Sprintf("%d file(s), %d binary, %d changed lines. Confirm they were regenerated by the tool that owns them and not edited by hand; binary content cannot be reviewed as text.", len(files), binaries, lines)),
	}}
}

// ---- tests ----------------------------------------------------------------------------

var testPathRE = regexp.MustCompile(`(^|/)(tests?|specs?|__tests__|testdata)/|_test\.go$|(^|/)test_[^/]+\.py$|_test\.py$|\.(test|spec)\.(ts|tsx|js|jsx|mjs)$|(Test|Tests|IT)\.(java|kt)$|_spec\.rb$|(^|/)tests\.rs$`)

func isTestFile(p string) bool { return testPathRE.MatchString(p) }

var rustTestRE = regexp.MustCompile(`#\[(cfg\()?test\b|\bmod\s+tests\b`)

func testFindings(files []agwp.FileDiff, s Settings) []Finding {
	var sources []agwp.FileDiff
	added := 0
	for _, f := range files {
		if isTestFile(f.Path) {
			return nil // the change includes tests; matching them to sources is not worth nagging about
		}
		if path.Ext(f.Path) == ".rs" && rustTestRE.MatchString(addedText(f)) {
			return nil // Rust keeps unit tests next to the code
		}
		if f.Status != "deleted" && s.Sources.Match(f.Path) {
			sources = append(sources, f)
			added += f.Additions
		}
	}
	if len(sources) == 0 || added < minSourceLines {
		return nil
	}
	names := make([]string, 0, len(sources))
	for _, f := range sources {
		names = append(names, fmt.Sprintf("`%s` (+%d)", f.Path, f.Additions))
	}
	sort.Strings(names)
	return []Finding{{
		Kind: KindTests, Severity: SevInfo,
		Title:   "Source changes without test changes",
		Details: append(clipList(names, 8), fmt.Sprintf("%d added source line(s) and no test file changed in this proposal. The check is a naming-convention heuristic; ignore it where tests do not apply.", added)),
	}}
}

// ---- secrets ---------------------------------------------------------------------------

type addedLine struct {
	No   int
	Text string
}

// addedLines extracts the added lines of a unified diff with their line
// numbers in the new file.
func addedLines(patch string) []addedLine {
	var out []addedLine
	line := 0
	for _, l := range strings.Split(patch, "\n") {
		switch {
		case strings.HasPrefix(l, "@@"):
			// @@ -a,b +c,d @@
			if i := strings.Index(l, "+"); i >= 0 {
				rest := l[i+1:]
				if j := strings.IndexAny(rest, ", "); j >= 0 {
					if n, err := strconv.Atoi(rest[:j]); err == nil {
						line = n - 1
					}
				}
			}
		case strings.HasPrefix(l, "+++"), strings.HasPrefix(l, "---"):
		case strings.HasPrefix(l, "+"):
			line++
			out = append(out, addedLine{No: line, Text: l[1:]})
		case strings.HasPrefix(l, "-"), strings.HasPrefix(l, `\`):
		default:
			line++
		}
	}
	return out
}

func addedText(f agwp.FileDiff) string {
	var b strings.Builder
	for _, l := range addedLines(f.Patch) {
		b.WriteString(l.Text)
		b.WriteByte('\n')
	}
	return b.String()
}

type secretRule struct {
	name string
	re   *regexp.Regexp
	// group is the capture group holding the secret value (0 = whole match).
	group int
	// generic rules additionally need entropy and must not look like placeholders.
	generic bool
}

var secretRules = []secretRule{
	{name: "private key block", re: regexp.MustCompile(`-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----`)},
	{name: "AWS access key id", re: regexp.MustCompile(`\b(?:AKIA|ASIA)[0-9A-Z]{16}\b`)},
	{name: "GitHub token", re: regexp.MustCompile(`\bgh[pousr]_[A-Za-z0-9]{36,}\b`)},
	{name: "GitHub fine-grained token", re: regexp.MustCompile(`\bgithub_pat_[A-Za-z0-9_]{50,}\b`)},
	{name: "Slack token", re: regexp.MustCompile(`\bxox[baprs]-[A-Za-z0-9-]{10,}\b`)},
	{name: "Stripe key", re: regexp.MustCompile(`\b[sr]k_live_[A-Za-z0-9]{16,}\b`)},
	{name: "Google API key", re: regexp.MustCompile(`\bAIza[0-9A-Za-z_\-]{35}\b`)},
	{name: "JSON web token", re: regexp.MustCompile(`\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b`)},
	{name: "npm token", re: regexp.MustCompile(`\bnpm_[A-Za-z0-9]{36}\b`)},
	{name: "SendGrid key", re: regexp.MustCompile(`\bSG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}\b`)},
	{name: "AgentGit credential", re: regexp.MustCompile(`\bagt_[0-9a-f]{16}_[A-Za-z0-9_-]{32,}\b`)},
	{name: "hard-coded credential", generic: true, group: 2,
		re: regexp.MustCompile(`(?i)\b(\w*(?:password|passwd|secret|api[_-]?key|access[_-]?key|private[_-]?key|auth[_-]?token|token)\w*)["']?\s*[:=]\s*["']([^"'\s]{16,})["']`)},
}

var (
	uuidRE        = regexp.MustCompile(`(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)
	placeholderRE = regexp.MustCompile(`(?i)example|sample|placeholder|changeme|change[-_]me|your[-_ ]|dummy|fake|redacted|xxxxx|\*\*\*\*|todo|insert|replace|<|>|\$\{|\{\{|%[sdv]|%\(|ghp_x|secret_here|not[-_]a[-_]real|(^|[-_.])(test|mock|stub|demo)([-_.]|$)|1234567|abcdefg|0123456`)
	// identifiers that name or locate a secret rather than hold one
	identifierSuffixRE = regexp.MustCompile(`(?i)(name|path|url|uri|file|dir|id|ref|type|kind|version|header|endpoint|env|field|label|prefix|suffix|format|mode)$`)
	pathWordRE         = regexp.MustCompile(`^[a-z][a-z-]*$`)
)

func entropy(s string) float64 {
	counts := map[rune]int{}
	n := 0
	for _, r := range s {
		counts[r]++
		n++
	}
	if n == 0 {
		return 0
	}
	h := 0.0
	for _, c := range counts {
		p := float64(c) / float64(n)
		h -= p * math.Log2(p)
	}
	return h
}

func hasDigitAndLetter(s string) bool {
	d, l := false, false
	for _, r := range s {
		switch {
		case r >= '0' && r <= '9':
			d = true
		case r >= 'a' && r <= 'z', r >= 'A' && r <= 'Z':
			l = true
		}
	}
	return d && l
}

// looksSecret applies the extra conditions that keep generic matches
// conservative: the value must not be a placeholder, an identifier or a
// resource path, must not repeat the name of the variable it is assigned to,
// and must look random.
func looksSecret(name, value string) bool {
	if placeholderRE.MatchString(value) || uuidRE.MatchString(value) || identifierSuffixRE.MatchString(name) {
		return false
	}
	if strings.Contains(strings.ToLower(value), strings.ToLower(strings.Trim(name, "_-"))) {
		return false
	}
	if isResourcePath(value) {
		return false
	}
	h := entropy(value)
	switch {
	case hasDigitAndLetter(value) && h >= 3.5:
		return true
	case len(value) >= 32 && h >= 4.2: // long random-looking text without digits
		return true
	}
	return false
}

// isResourcePath: "projects/123/secrets/db-password/versions/latest" and the
// like are locations, not credentials.
func isResourcePath(v string) bool {
	segs := strings.Split(v, "/")
	if len(segs) < 3 {
		return false
	}
	words := 0
	for _, s := range segs {
		if pathWordRE.MatchString(s) {
			words++
		}
	}
	return words*2 >= len(segs)
}

// redact shows enough of a value to find it, not enough to use it.
func redact(v string) string {
	if len(v) <= 6 {
		return fmt.Sprintf("(%d chars)", len(v))
	}
	return fmt.Sprintf("`%s…` (%d chars)", v[:4], len(v))
}

const allowMarker = "ag:allow-secret"

func secretFindings(files []agwp.FileDiff, s Settings) []Finding {
	var hits []string
	for _, f := range files {
		if f.Binary || s.Generated.Match(f.Path) {
			continue
		}
		for _, l := range addedLines(f.Patch) {
			if strings.Contains(l.Text, allowMarker) || allowed(l.Text, s.Allow) {
				continue
			}
			for _, rule := range secretRules {
				m := rule.re.FindStringSubmatch(l.Text)
				if m == nil {
					continue
				}
				value := m[rule.group]
				if strings.Contains(value, "EXAMPLE") {
					continue
				}
				if rule.generic && !looksSecret(m[1], value) {
					continue
				}
				desc := rule.name
				if rule.group == 0 && rule.name != "private key block" {
					desc += " " + redact(value)
				} else if rule.generic {
					desc += " " + redact(value)
				}
				hits = append(hits, fmt.Sprintf("`%s:%d` — %s", f.Path, l.No, desc))
				break // one finding per line
			}
		}
	}
	if len(hits) == 0 {
		return nil
	}
	return []Finding{{
		Kind: KindSecret, Severity: SevHigh,
		Title: "Possible secret in added lines",
		Details: append(clipList(hits, 10),
			"If this is real, revoke it now: removing it from this proposal does not remove it from the history other people have already fetched. If it is a test value, mark the line with `"+allowMarker+"` or add a pattern to the repository's allow list."),
	}}
}

func allowed(text string, allow []*regexp.Regexp) bool {
	for _, re := range allow {
		if re.MatchString(text) {
			return true
		}
	}
	return false
}
