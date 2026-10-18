package review

import (
	"context"
	"fmt"
	"regexp"
	"sort"
	"strings"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
)

// KindModel labels the findings a language model wrote.
const KindModel = "model_review"

var privateKeyBlockRE = regexp.MustCompile(`-----BEGIN (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----[\s\S]*?(?:-----END (?:RSA |EC |DSA |OPENSSH |PGP |ENCRYPTED )?PRIVATE KEY(?: BLOCK)?-----|\z)`)

// RedactSecrets masks everything in text that the secret scanner would flag, and says how many values it masked. It is
// applied to text before that text leaves the machine for a model provider: a credential that was committed by
// mistake must not also be sent to a third party.
func RedactSecrets(text string) (string, int) {
	n := 0
	text = privateKeyBlockRE.ReplaceAllStringFunc(text, func(string) string { n++; return "[REDACTED private key block]" })
	for _, rule := range secretRules {
		if rule.name == "private key block" {
			continue
		}
		if !rule.generic {
			text = rule.re.ReplaceAllStringFunc(text, func(string) string { n++; return "[REDACTED " + rule.name + "]" })
			continue
		}
		text = rule.re.ReplaceAllStringFunc(text, func(m string) string {
			sub := rule.re.FindStringSubmatch(m)
			if sub == nil || !looksSecret(sub[1], sub[rule.group]) {
				return m
			}
			n++
			return strings.Replace(m, sub[rule.group], "[REDACTED]", 1)
		})
	}
	return text, n
}

// HasSecret reports whether the scanner would flag anything in text.
func HasSecret(text string) bool {
	_, n := RedactSecrets(text)
	return n > 0
}

// Change is what a model reviewer is shown: the proposal and the per-file diffs the heuristic reviewers already read.
type Change struct {
	Proposal agwp.Proposal
	Files    []agwp.FileDiff
	Summary  Summary
}

// ModelReview is a model reviewer's answer.
type ModelReview struct {
	// Summary is one or two sentences about the change, in the model's words.
	Summary  string
	Findings []Finding
	// Notes say what the model was not shown (files cut for size) so that the comment can tell the reader.
	Notes []string
	// Model is the model that answered, as the provider reported it.
	Model string
}

// ModelReviewer reads a change and PROPOSES findings. Whatever it returns is rendered as a clearly marked section
// of a comment review; nothing it says is a check, an approval or a request for changes.
type ModelReviewer interface {
	// Name identifies the reviewer in comments ("model/<id>").
	Name() string
	Review(ctx context.Context, c Change) (*ModelReview, error)
}

// LLMReviewer is the model-backed ModelReviewer.
//
// The model is shown the title and description of the proposal and a bounded part of the diff, all of it fenced as
// untrusted data (llm.Guard). It must answer with one JSON object; every field is validated, a finding that names a
// file that is not in the change loses its location, the number and the length of findings are bounded, and anything
// that is not the expected shape is an error (the heuristic review is posted without it and says why).
type LLMReviewer struct {
	LLM llm.Completer
	// DiffBytes bounds the patch text placed in the prompt (default 40000); files that do not fit are listed as not shown.
	DiffBytes int
	// MaxFindings bounds the findings taken from one answer (default 8).
	MaxFindings int
}

var _ ModelReviewer = LLMReviewer{}

// Name implements ModelReviewer.
func (r LLMReviewer) Name() string { return "model/" + r.LLM.Model() }

const reviewSystem = `You are a careful senior engineer reviewing a change that someone proposes to merge into a source code repository. A person will read your findings before deciding anything; you cannot approve, reject or merge.

Report only concrete problems you can see in the diff: bugs, security problems, broken error handling, behaviour changes that the title and description do not mention, tests that are missing for new behaviour. Do not speculate, do not restate the change, do not comment on style, and report nothing if you find nothing.

Reply with a single JSON object and nothing else, no Markdown fence, with exactly these keys:
  "summary": one or two sentences saying what the change does,
  "findings": a list (empty when there is nothing to report) of objects with the keys
     "severity": "high" (a likely bug or a security problem), "warning" (worth a look) or "info",
     "title": a short title,
     "details": one or two sentences explaining the problem and where it is,
     "path": the path of the file concerned, or "" when it concerns the change as a whole,
     "line": a line number in the new version of the file, or 0.

%s`

type modelAnswer struct {
	Summary  string `json:"summary"`
	Findings []struct {
		Severity string `json:"severity"`
		Title    string `json:"title"`
		Details  string `json:"details"`
		Path     string `json:"path"`
		Line     int    `json:"line"`
	} `json:"findings"`
}

// Review implements ModelReviewer.
func (r LLMReviewer) Review(ctx context.Context, c Change) (*ModelReview, error) {
	budget := r.DiffBytes
	if budget <= 0 {
		budget = 40_000
	}
	maxFindings := r.MaxFindings
	if maxFindings <= 0 {
		maxFindings = 8
	}
	g := llm.NewGuard()
	var user strings.Builder
	user.WriteString("Review this proposed change.\n\n")
	redacted := 0
	mask := func(text string) string {
		out, n := RedactSecrets(text)
		redacted += n
		return out
	}
	user.WriteString(g.Wrap("title of the proposal", mask(c.Proposal.Title), 300))
	if d := strings.TrimSpace(c.Proposal.Description); d != "" {
		user.WriteString(g.Wrap("description of the proposal", mask(d), 2000))
	}
	files := append([]agwp.FileDiff(nil), c.Files...)
	sort.Slice(files, func(i, j int) bool { return files[i].Path < files[j].Path })
	known := map[string]bool{}
	var shown, hidden []string
	used := 0
	for _, f := range files {
		known[f.Path] = true
		if f.Binary || f.Patch == "" {
			hidden = append(hidden, f.Path+" ("+f.Status+", no text diff)")
			continue
		}
		if used+len(f.Patch) > budget {
			hidden = append(hidden, f.Path+" ("+f.Status+", does not fit)")
			continue
		}
		used += len(f.Patch)
		shown = append(shown, f.Path)
		user.WriteString(g.Wrap(fmt.Sprintf("diff of %s (%s, +%d -%d)", f.Path, f.Status, f.Additions, f.Deletions), mask(f.Patch), 0))
	}
	var notes []string
	if len(hidden) > 0 {
		list := hidden
		if len(list) > 20 {
			list = append(list[:20:20], fmt.Sprintf("and %d more", len(hidden)-20))
		}
		notes = append(notes, fmt.Sprintf("the model was not shown %d file(s): %s", len(hidden), strings.Join(list, ", ")))
		user.WriteString(g.Wrap("files that were not shown to you", strings.Join(list, "\n"), 4000))
	}
	if c.Summary.Truncated {
		notes = append(notes, "the server truncated the diff, so the model saw only part of the change")
	}
	if redacted > 0 {
		notes = append(notes, fmt.Sprintf("%d value(s) that look like secrets were masked before the diff was sent to the model", redacted))
	}
	if len(shown) == 0 {
		return nil, fmt.Errorf("there is no text diff to show a model (%d file(s) are binary or too large)", len(hidden))
	}
	resp, err := r.LLM.Complete(ctx, llm.Request{Purpose: "review", Messages: []llm.Message{
		{Role: llm.System, Content: fmt.Sprintf(reviewSystem, g.Notice())},
		{Role: llm.User, Content: user.String()},
	}})
	if err != nil {
		return nil, err
	}
	var ans modelAnswer
	if err := llm.Decode(resp.Text, &ans); err != nil {
		return nil, err
	}
	out := &ModelReview{Summary: llm.Clip(ans.Summary, 500), Notes: notes, Model: resp.Model}
	if out.Model == "" {
		out.Model = r.LLM.Model()
	}
	for _, f := range ans.Findings {
		if len(out.Findings) >= maxFindings {
			out.Notes = append(out.Notes, fmt.Sprintf("the model reported more than %d findings; the rest are not shown", maxFindings))
			break
		}
		sev := strings.ToLower(strings.TrimSpace(f.Severity))
		switch sev {
		case "high":
			sev = SevHigh
		case "warning", "warn":
			sev = SevWarn
		case "info":
			sev = SevInfo
		default:
			return nil, fmt.Errorf("a finding has an invalid severity %q", llm.Clip(f.Severity, 20))
		}
		title := llm.Clip(f.Title, 120)
		if title == "" {
			return nil, fmt.Errorf("a finding has no title")
		}
		details := []string{}
		if d := llm.Clip(f.Details, 500); d != "" {
			details = append(details, d)
		}
		loc := ""
		if p, ok := resolvePath(known, strings.TrimSpace(f.Path)); p != "" || !ok {
			if ok {
				loc = p
				if f.Line > 0 && f.Line < 10_000_000 {
					loc = fmt.Sprintf("%s:%d", p, f.Line)
				}
				details = append(details, "where: `"+loc+"`")
			} else {
				details = append(details, "the model named a file that is not part of this change; its location was dropped")
			}
		}
		out.Findings = append(out.Findings, Finding{Kind: KindModel, Severity: sev, Title: title, Details: details})
	}
	return out, nil
}

// resolvePath maps the path a model names onto a path of the change: exactly, without a diff prefix (a/, b/, ./),
// or as the tail of exactly one changed path ("users.go" for "store/users.go"). An empty path is no path (and
// ok); a path that matches nothing, or is ambiguous, is not located (ok false).
func resolvePath(known map[string]bool, p string) (string, bool) {
	if p == "" {
		return "", true
	}
	for _, pre := range []string{"a/", "b/", "./"} {
		if !known[p] && strings.HasPrefix(p, pre) {
			p = strings.TrimPrefix(p, pre)
		}
	}
	if known[p] {
		return p, true
	}
	found := ""
	for k := range known {
		if strings.HasSuffix(k, "/"+p) {
			if found != "" {
				return p, false // ambiguous
			}
			found = k
		}
	}
	if found != "" {
		return found, true
	}
	return p, false
}
