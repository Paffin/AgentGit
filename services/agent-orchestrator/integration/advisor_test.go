//go:build linux

package integration

import (
	"context"
	"encoding/json"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/advisor"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

const libBase = `package lib

import (
	"fmt"
)

func Hello() { fmt.Println("hi") }
`

const configBase = "package lib\n\nconst Timeout = 30\n"

func advisorReviews(w *world, proposal string) []agwp.Review {
	d, err := w.alice.GetProposal(context.Background(), w.repo, proposal)
	if err != nil {
		w.t.Fatal(err)
	}
	var out []agwp.Review
	for _, r := range d.Reviews {
		if r.Reviewer == "service:advisor" {
			out = append(out, r)
		}
	}
	return out
}

// Two workspaces edit the same files. One file's conflict is mechanical (both
// sides added an import), the other is a real disagreement. The advisor posts a
// comment on the second proposal saying so and offers the merged file for the
// first; applying that suggestion through the ordinary workspace sync lets the
// proposal land. The advisor never approves and never changes anything itself.
func TestAdvisorSuggestsResolutionsForConflictingWorkspaces(t *testing.T) {
	w := newWorld(t, worldOptions{})
	ctx := context.Background()
	w.seed(map[string]string{"src/lib.go": libBase, "src/config.go": configBase})

	one, _ := w.propose("ws-one", "Add os and a longer timeout",
		put("src/lib.go", strings.Replace(libBase, "\t\"fmt\"\n", "\t\"fmt\"\n\t\"os\"\n", 1)),
		put("src/config.go", strings.Replace(configBase, "30", "60", 1)))
	// two is made from the same base, before one lands
	two, wsTwo := w.propose("ws-two", "Add strings and another timeout",
		put("src/lib.go", strings.Replace(libBase, "\t\"fmt\"\n", "\t\"fmt\"\n\t\"strings\"\n", 1)),
		put("src/config.go", strings.Replace(configBase, "30", "90", 1)))

	o := w.startOrchestrator(orchConfig{
		Repo: "    schedule: {enabled: false}\n    review: {enabled: false}\n    index: {enabled: false}\n",
	})
	o.waitReady()

	// Before anything landed there is nothing to advise on.
	time.Sleep(1500 * time.Millisecond)
	if rs := advisorReviews(w, two.ID); len(rs) != 0 {
		t.Fatalf("advice without a conflict: %+v", rs)
	}
	w.land(one)

	var review agwp.Review
	eventually(t, "the advisor to comment on the conflicting proposal", 30*time.Second, func() bool {
		rs := advisorReviews(w, two.ID)
		if len(rs) > 0 {
			review = rs[0]
		}
		return len(rs) > 0
	})
	if review.Decision != agwp.DecisionComment {
		t.Fatalf("the advisor must only comment, got %q", review.Decision)
	}
	for _, want := range []string{"advice only", "`src/lib.go`", "both sides extended the same list", "`src/config.go`", "needs a person", advisor.MarkerPrefix} {
		if !strings.Contains(review.Body, want) {
			t.Errorf("the comment lacks %q:\n%s", want, review.Body)
		}
	}
	if review.Head != two.Head {
		t.Errorf("the comment is about head %s, the proposal's head is %s", review.Head, two.Head)
	}

	// The same advice is available, whole, from the admin API.
	code, body := o.get("/advisor/suggestions?proposal=" + two.ID)
	if code != 200 {
		t.Fatalf("/advisor/suggestions = %d %s", code, body)
	}
	var rec advisor.Record
	if err := json.Unmarshal([]byte(body), &rec); err != nil {
		t.Fatal(err)
	}
	if !rec.Posted || len(rec.Suggestions) != 2 {
		t.Fatalf("record = %+v", rec)
	}
	byPath := map[string]advisor.Suggestion{}
	for _, s := range rec.Suggestions {
		byPath[s.Path] = s
	}
	lib, cfg := byPath["src/lib.go"], byPath["src/config.go"]
	if lib.Class != advisor.ClassUnion || !lib.Complete {
		t.Fatalf("lib.go suggestion = %+v", lib)
	}
	merged := string(lib.Resolved)
	if strings.Count(merged, "\t\"os\"\n") != 1 || strings.Count(merged, "\t\"strings\"\n") != 1 || strings.Count(merged, "\t\"fmt\"\n") != 1 ||
		!strings.HasPrefix(merged, "package lib\n\nimport (\n") || !strings.HasSuffix(merged, ")\n\nfunc Hello() { fmt.Println(\"hi\") }\n") {
		t.Fatalf("the suggested file is not the union of both import lists:\n%s", merged)
	}
	if cfg.Class != advisor.ClassNeedsPerson || cfg.Complete || len(cfg.Resolved) != 0 {
		t.Fatalf("config.go must be left to a person: %+v", cfg)
	}

	// The advisor changed nothing: the proposal is untouched, the stream is where one left it, and the advice is posted once.
	got, err := w.alice.GetProposal(ctx, w.repo, two.ID)
	if err != nil || got.Proposal.State != agwp.ProposalOpen || got.Proposal.Head != two.Head {
		t.Fatalf("proposal after advice: %+v %v", got, err)
	}
	if res, err := w.carol.Integrate(ctx, w.repo, two.ID, nil); err != nil || res.Outcome.Status == "integrated" {
		t.Fatalf("a conflicted proposal must not integrate on its own: %+v %v", res, err)
	}
	time.Sleep(2500 * time.Millisecond)
	if rs := advisorReviews(w, two.ID); len(rs) != 1 {
		t.Fatalf("the advice must be posted once, got %d comments", len(rs))
	}

	// A person (here: alice) applies the suggestion and decides the rest, through the normal path.
	res, err := w.alice.SyncWorkspace(ctx, w.repo, wsTwo.Name, []agwp.FileChange{
		put("src/lib.go", merged),
		put("src/config.go", "package lib\n\nconst Timeout = 75\n"),
	})
	if err != nil || res.Status != "merged" {
		t.Fatalf("sync = %+v %v", res, err)
	}
	w.updateProposal(two.ID, res.Head)
	w.land(two)
	if got, _ := w.file("main", "src/lib.go"); got != merged {
		t.Errorf("main:src/lib.go =\n%s\nwant the advisor's suggestion", got)
	}
	if got, _ := w.file("main", "src/config.go"); got != "package lib\n\nconst Timeout = 75\n" {
		t.Errorf("main:src/config.go = %q", got)
	}

	// Once resolved the advice disappears from the orchestrator's memory.
	eventually(t, "the advisor to forget a resolved conflict", 20*time.Second, func() bool {
		code, _ := o.get("/advisor/suggestions?proposal=" + two.ID)
		return code == 404
	})
	for _, r := range advisorReviews(w, two.ID) {
		if r.Decision != agwp.DecisionComment {
			t.Errorf("advisor decision %q", r.Decision)
		}
	}
}
