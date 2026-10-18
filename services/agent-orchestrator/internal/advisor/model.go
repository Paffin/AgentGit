package advisor

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
	"unicode/utf8"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/review"
)

// ClassModel is a suggestion written by a language model.
const ClassModel = "model_proposed"

// ModelAdvisor asks a language model for a resolution of one conflicting file. It is built to be distrusted:
//
//   - The deterministic rules run first. A mechanical resolution they rate with high confidence is used as it is
//     (it costs nothing and cannot hallucinate); binary files and delete/modify conflicts are not a question for a
//     model at all. Only the rest is sent.
//   - What is sent is bounded and fenced: the three versions of one file, each cut to PromptFileBytes, wrapped as
//     untrusted data (llm.Guard). The system prompt says that the file contents may contain instructions and must not
//     be obeyed.
//   - The answer must be one JSON object of a fixed shape. It is validated: a known confidence, valid UTF-8 text of a
//     bounded size without conflict markers. Anything else is rejected, and then the rules' answer stands, with a
//     note saying that the model was asked and why its answer was not used.
//   - The result is still only advice. Its confidence is never rated above "medium" (that word describes how
//     mechanical a resolution is, and a model's is not), the comment says it is written by a model, and lines the
//     suggestion drops from either side, or invents, are counted in the notes for the person who reads it.
//
// It has no way to apply anything: a suggestion enters the repository only when an agent or a person submits it.
type ModelAdvisor struct {
	LLM   llm.Completer
	Rules Rules
	// PromptFileBytes bounds each of the three versions placed in the prompt (default 24000). A file larger than this
	// is left to the rules.
	PromptFileBytes int
}

var _ Advisor = ModelAdvisor{}

// Name implements Advisor.
func (a ModelAdvisor) Name() string { return "model/" + a.LLM.Model() + "+" + a.Rules.Name() }

// Nondeterministic: a model's answer may differ from run to run, so the advisor's comments are recognised by the
// conflict they answer, not by their text.
func (ModelAdvisor) Nondeterministic() bool { return true }

type modelAnswer struct {
	Resolution  *string `json:"resolution"`
	Confidence  string  `json:"confidence"`
	Explanation string  `json:"explanation"`
	NeedsPerson bool    `json:"needs_person"`
}

// Suggest implements Advisor.
func (a ModelAdvisor) Suggest(ctx context.Context, in Input) (Suggestion, error) {
	h, err := a.Rules.Suggest(ctx, in)
	if err != nil {
		return h, err
	}
	switch {
	case h.Class == ClassBinary || h.Class == ClassDeleteEdit:
		return h, nil // not a question about text merging
	case h.Complete && h.Confidence == High:
		return h, nil // a mechanical resolution the rules are sure of
	}
	limit := a.PromptFileBytes
	if limit <= 0 {
		limit = 24_000
	}
	for _, f := range []*File{in.Base, in.Ours, in.Theirs} {
		if f != nil && len(f.Content) > limit {
			h.Notes = append(h.Notes, fmt.Sprintf("a language model was not asked: a version of the file is larger than %d bytes", limit))
			return h, nil
		}
		if f != nil && review.HasSecret(string(f.Content)) {
			// masking would change the file and so the merge; sending a credential to a third party is not an option
			h.Notes = append(h.Notes, "a language model was not asked: the file contains something that looks like a secret, and file contents with secrets in them are not sent to a model provider")
			return h, nil
		}
	}
	s, err := a.ask(ctx, in, h)
	if err != nil {
		var note string
		switch {
		case errors.Is(err, llm.ErrBudget):
			note = "a language model was not asked: the configured budget is used up (" + err.Error() + ")"
		default:
			note = "a language model was asked and its answer was not used: " + llm.Clip(err.Error(), 300)
		}
		h.Notes = append(h.Notes, note)
		return h, nil
	}
	return s, nil
}

const adviceSystem = `You are a careful senior software engineer helping to resolve a merge conflict in ONE file.

You are given three versions of the file: BASE (the common ancestor), OURS (the version on the stream the change is meant to land on) and THEIRS (the version in the proposed change). Produce the merged file that keeps the intent of both sides.

Rules:
- Keep every intended change of both sides. Do not add unrelated changes, do not reformat code you were not asked to touch, do not invent new functionality.
- If the two sides contradict each other and a merge needs a human decision, say so: set "needs_person" to true and, if you can, still give your best attempt.
- Reply with a single JSON object and nothing else, no Markdown fence, with exactly these keys:
  "resolution": the complete merged file as a string, or null when you cannot give one,
  "confidence": "low", "medium" or "high" (how sure you are that the merged file is correct),
  "explanation": at most three sentences saying what you did and what a reviewer should check,
  "needs_person": true or false.

%s`

func (a ModelAdvisor) ask(ctx context.Context, in Input, h Suggestion) (Suggestion, error) {
	limit := a.PromptFileBytes
	if limit <= 0 {
		limit = 24_000
	}
	g := llm.NewGuard()
	var user strings.Builder
	fmt.Fprintf(&user, "Resolve the conflict in the file below. The server reported it as: %s.\n", llm.Clip(in.Kind, 200))
	if len(h.Notes) > 0 || h.Class != "" {
		fmt.Fprintf(&user, "A rule-based analysis classified it as %q with these notes: %s.\n", h.Class, llm.Clip(strings.Join(h.Notes, " "), 600))
	}
	user.WriteString("\n")
	for _, v := range []struct {
		label string
		f     *File
	}{{"BASE (common ancestor)", in.Base}, {"OURS (the stream)", in.Ours}, {"THEIRS (the proposed change)", in.Theirs}} {
		if v.f == nil {
			fmt.Fprintf(&user, "%s: the file does not exist in this version.\n\n", v.label)
			continue
		}
		user.WriteString(g.Wrap(v.label+" of "+in.Path, string(v.f.Content), limit))
		user.WriteString("\n")
	}
	resp, err := a.LLM.Complete(ctx, llm.Request{Purpose: "advisor", Messages: []llm.Message{
		{Role: llm.System, Content: fmt.Sprintf(adviceSystem, g.Notice())},
		{Role: llm.User, Content: user.String()},
	}})
	if err != nil {
		return h, err
	}
	var ans modelAnswer
	if err := llm.Decode(resp.Text, &ans); err != nil {
		return h, err
	}
	return a.validate(in, h, ans, resp)
}

// validate turns a model's answer into a Suggestion, or refuses it.
func (a ModelAdvisor) validate(in Input, h Suggestion, ans modelAnswer, resp *llm.Response) (Suggestion, error) {
	if !llm.OneOf(ans.Confidence, Low, Medium, High) {
		return h, fmt.Errorf("the answer has an invalid confidence %q", llm.Clip(ans.Confidence, 20))
	}
	maxBytes := a.Rules.MaxFileBytes
	if maxBytes <= 0 {
		maxBytes = 1 << 20
	}
	s := Suggestion{Path: in.Path, Kind: in.Kind, Advisor: "model/" + a.LLM.Model(), Class: ClassModel, Regions: h.Regions}
	s.Notes = append(s.Notes, "written by a language model ("+llm.Clip(a.modelID(resp), 80)+"), not by rules: a proposal that nothing has checked")
	if e := llm.Clip(ans.Explanation, 600); e != "" {
		s.Notes = append(s.Notes, "the model says: "+e)
	}
	if len(h.Notes) > 0 {
		s.Notes = append(s.Notes, "rule-based analysis: "+h.Class+"; "+strings.Join(h.Notes, "; "))
	}
	if ans.NeedsPerson || ans.Resolution == nil {
		s.Class = ClassNeedsPerson
		if ans.NeedsPerson {
			s.Notes = append(s.Notes, "the model says this needs a person's decision")
		}
		return s, nil
	}
	res := *ans.Resolution
	switch {
	case !utf8.ValidString(res) || strings.ContainsRune(res, 0):
		return h, errors.New("the proposed file is not valid text")
	case len(res) > maxBytes:
		return h, fmt.Errorf("the proposed file is larger than %d bytes", maxBytes)
	case strings.TrimSpace(res) == "":
		return h, errors.New("the proposed file is empty")
	case hasConflictMarkers(res) && !(in.Base != nil && hasConflictMarkers(string(in.Base.Content))):
		return h, errors.New("the proposed file still contains conflict markers")
	}
	s.Resolved = []byte(res)
	s.Complete = true
	conf := ans.Confidence
	if conf == High {
		conf = Medium // confidence describes how mechanical a resolution is; a model's never is
	}
	s.Confidence = conf
	sum := sha256.Sum256(s.Resolved)
	s.SHA256 = hex.EncodeToString(sum[:])
	s.Notes = append(s.Notes, auditResolution(in, res)...)
	return s, nil
}

func (a ModelAdvisor) modelID(resp *llm.Response) string {
	if resp != nil && resp.Model != "" {
		return resp.Model
	}
	return a.LLM.Model()
}

func hasConflictMarkers(s string) bool {
	for _, l := range strings.Split(s, "\n") {
		if strings.HasPrefix(l, "<<<<<<< ") || strings.HasPrefix(l, ">>>>>>> ") || strings.TrimRight(l, "\r") == "=======" {
			return true
		}
	}
	return false
}

// auditResolution compares the proposed file with the three versions it came from, line by line, and reports what a
// reader should look at: lines one side added that are missing from the result, and lines in the result that none of
// the versions has. (A good resolution can have either; a bad one usually does. It is information, not a verdict.)
func auditResolution(in Input, res string) []string {
	lines := func(f *File) map[string]bool {
		set := map[string]bool{}
		if f != nil {
			for _, l := range strings.Split(string(f.Content), "\n") {
				if t := strings.TrimSpace(l); t != "" {
					set[t] = true
				}
			}
		}
		return set
	}
	base, ours, theirs := lines(in.Base), lines(in.Ours), lines(in.Theirs)
	result := map[string]bool{}
	for _, l := range strings.Split(res, "\n") {
		if t := strings.TrimSpace(l); t != "" {
			result[t] = true
		}
	}
	missing := func(side map[string]bool) int {
		n := 0
		for l := range side {
			if !base[l] && !result[l] {
				n++
			}
		}
		return n
	}
	invented := 0
	for l := range result {
		if !base[l] && !ours[l] && !theirs[l] {
			invented++
		}
	}
	var notes []string
	if n := missing(ours); n > 0 {
		notes = append(notes, fmt.Sprintf("check: %d line(s) the stream added are missing from this suggestion", n))
	}
	if n := missing(theirs); n > 0 {
		notes = append(notes, fmt.Sprintf("check: %d line(s) the proposal added are missing from this suggestion", n))
	}
	if invented > 0 {
		notes = append(notes, fmt.Sprintf("check: %d line(s) of this suggestion appear in none of the three versions", invented))
	}
	if len(notes) == 0 {
		notes = append(notes, "every line either side added is in this suggestion, and it adds no line of its own")
	}
	return notes
}
