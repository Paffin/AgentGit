package semindex

import (
	"context"
	"fmt"
	"sort"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/review"
)

// Verdicts of an Assessment.
const (
	VerdictConflict   = "conflict"
	VerdictCompatible = "compatible"
	VerdictUnsure     = "unsure"
)

// Assessment is a language model's opinion on an overlap the index found: do the two proposals really conflict in
// meaning? It is attached to the overlap next to the index's own (static, rule-based) severity and never replaces it.
type Assessment struct {
	Model    string    `json:"model"`
	Verdict  string    `json:"verdict"`
	Severity string    `json:"severity"`
	Why      string    `json:"why"`
	At       time.Time `json:"at"`
	// Source says what wrote this: always "language model"; the static severity of the overlap is not model output.
	Source string `json:"source"`
}

// ProposalView is what a Judge is told about one of two proposals.
type ProposalView struct {
	ID, Title, Description string
	// Changed are the definitions the static analysis found changed; Patches the (bounded) diffs of the files that matter.
	Changed []Changed
	Patches map[string]string
}

// Pair is two open proposals the static analysis found overlapping.
type Pair struct {
	A, B     ProposalView
	Overlaps []Overlap
}

// Judge assesses overlapping proposals.
type Judge interface {
	// Name identifies the judge ("model/<id>").
	Name() string
	Judge(ctx context.Context, p Pair) (*Assessment, error)
}

// LLMJudge is the model-backed Judge. Like every model-backed component it sends only bounded, fenced, secret-masked
// text, accepts only a validated JSON answer of a fixed shape, and its output is information that sits beside the
// index's own findings: it blocks and approves nothing.
type LLMJudge struct {
	LLM llm.Completer
	// PatchBytes bounds the diff text placed in the prompt, in total for both proposals (default 30000).
	PatchBytes int
}

var _ Judge = LLMJudge{}

// Name implements Judge.
func (j LLMJudge) Name() string { return "model/" + j.LLM.Model() }

const judgeSystem = `You judge whether two proposed changes to the same source code repository conflict in MEANING, even where their text merges cleanly: for example one changes what a function promises while the other relies on the old behaviour, or both change the same configuration in incompatible ways.

You are given, for each proposal, its title and description, the definitions a static analysis found it changing, and the diffs of the files involved. A static analysis already found that the two overlap; your job is to say whether the overlap matters.

Reply with a single JSON object and nothing else, no Markdown fence, with exactly these keys:
  "verdict": "conflict" (merging both would be wrong or surprising), "compatible" (they touch the same things but fit together) or "unsure",
  "severity": "low", "medium" or "high" (how bad it would be if they were merged without anyone noticing; use "low" for compatible),
  "explanation": at most three sentences naming the definitions or lines that matter.

%s`

type judgeAnswer struct {
	Verdict     string `json:"verdict"`
	Severity    string `json:"severity"`
	Explanation string `json:"explanation"`
}

// Judge implements Judge.
func (j LLMJudge) Judge(ctx context.Context, p Pair) (*Assessment, error) {
	budget := j.PatchBytes
	if budget <= 0 {
		budget = 30_000
	}
	g := llm.NewGuard()
	var user strings.Builder
	redacted := 0
	mask := func(s string) string {
		out, n := review.RedactSecrets(s)
		redacted += n
		return out
	}
	user.WriteString("Two proposals overlap. What the static analysis found:\n")
	for _, o := range p.Overlaps {
		fmt.Fprintf(&user, "- %s (static severity %s): %s\n", o.Kind, o.Severity, o.Why)
	}
	user.WriteString("\n")
	for _, side := range []struct {
		label string
		v     ProposalView
	}{{"A", p.A}, {"B", p.B}} {
		user.WriteString(g.Wrap("proposal "+side.label+" title", mask(side.v.Title), 300))
		if d := strings.TrimSpace(side.v.Description); d != "" {
			user.WriteString(g.Wrap("proposal "+side.label+" description", mask(d), 1500))
		}
		var defs []string
		for _, c := range side.v.Changed {
			defs = append(defs, fmt.Sprintf("%s: %s %s (%s)", c.Path, c.Kind, c.Qualified(), c.Change))
		}
		if len(defs) > 60 {
			defs = append(defs[:60], fmt.Sprintf("and %d more", len(defs)-60))
		}
		user.WriteString(g.Wrap("proposal "+side.label+" changed definitions", strings.Join(defs, "\n"), 4000))
		paths := make([]string, 0, len(side.v.Patches))
		for path := range side.v.Patches {
			paths = append(paths, path)
		}
		sort.Strings(paths)
		each := budget / 2
		used := 0
		for _, path := range paths {
			patch := mask(side.v.Patches[path])
			if used+len(patch) > each {
				user.WriteString(g.Wrap("proposal "+side.label+" diff of "+path, "(not shown: the diff does not fit)", 0))
				continue
			}
			used += len(patch)
			user.WriteString(g.Wrap("proposal "+side.label+" diff of "+path, patch, 0))
		}
	}
	resp, err := j.LLM.Complete(ctx, llm.Request{Purpose: "index", Messages: []llm.Message{
		{Role: llm.System, Content: fmt.Sprintf(judgeSystem, g.Notice())},
		{Role: llm.User, Content: user.String()},
	}})
	if err != nil {
		return nil, err
	}
	var ans judgeAnswer
	if err := llm.Decode(resp.Text, &ans); err != nil {
		return nil, err
	}
	if !llm.OneOf(ans.Verdict, VerdictConflict, VerdictCompatible, VerdictUnsure) {
		return nil, fmt.Errorf("the answer has an invalid verdict %q", llm.Clip(ans.Verdict, 20))
	}
	if !llm.OneOf(ans.Severity, "low", "medium", "high") {
		return nil, fmt.Errorf("the answer has an invalid severity %q", llm.Clip(ans.Severity, 20))
	}
	sev := ans.Severity
	if ans.Verdict == VerdictCompatible {
		sev = "low"
	}
	why := llm.Clip(ans.Explanation, 600)
	if why == "" {
		return nil, fmt.Errorf("the answer has no explanation")
	}
	if redacted > 0 {
		why += fmt.Sprintf(" (%d secret-like value(s) were masked in the text the model saw.)", redacted)
	}
	model := resp.Model
	if model == "" {
		model = j.LLM.Model()
	}
	return &Assessment{Model: model, Verdict: ans.Verdict, Severity: sev, Why: why, Source: "language model"}, nil
}
