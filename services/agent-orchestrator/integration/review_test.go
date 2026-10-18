//go:build linux

package integration

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/review"
)

func reviewerComments(w *world, proposal string) []agwp.Review {
	d, err := w.Alice.GetProposal(context.Background(), w.Repo, proposal)
	if err != nil {
		w.T.Fatal(err)
	}
	var out []agwp.Review
	for _, r := range d.Reviews {
		if r.Reviewer == "service:reviewer" {
			out = append(out, r)
		}
	}
	return out
}

func reviewRecord(o *orchestrator, proposal string) (review.Record, bool) {
	for _, r := range o.status().Repos {
		if r.Review == nil {
			continue
		}
		for _, rec := range r.Review.Recent {
			if rec.Proposal == proposal {
				return rec, true
			}
		}
	}
	return review.Record{}, false
}

const goAdd = `package pkg

// Add returns the sum of a and b.
func Add(a, b int) int {
	return a + b
}
`

const goAddTest = `package pkg

import "testing"

func TestAdd(t *testing.T) {
	if Add(1, 2) != 3 {
		t.Fatal("1+2")
	}
}
`

// The deterministic reviewers say something about a planted fake secret, an
// oversized change and a protected path with untested code, say nothing about
// clean code, never approve, and never write the secret into their comment.
func TestReviewersFlagRiskyChangesAndLeaveCleanCodeAlone(t *testing.T) {
	w := newWorld(t, worldOptions{})
	ctx := context.Background()
	w.Seed(map[string]string{"README.md": "# web\n"})

	// A token-shaped value assembled at run time, so this file holds no secret-looking text.
	fakeToken := "ghp" + "_" + randHex(18)
	if len(fakeToken) != 40 {
		t.Fatalf("test token has the wrong shape: %q", fakeToken)
	}

	clean, _ := w.Propose("ws-clean", "Add Add with its test", put("pkg/add.go", goAdd), put("pkg/add_test.go", goAddTest))
	leaky, _ := w.Propose("ws-leaky", "Wire up the deploy settings",
		put("deploy/settings.txt", "region = eu-west-1\nreleaser = "+fakeToken+"\nreplicas = 3\n"))

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule: {enabled: false}\n    advisor: {enabled: false}\n    index: {enabled: false}\n    review:\n      max_files: 5\n      max_lines: 100\n",
		Executors: "",
	})
	o.waitReady()

	// Opened before the service started: found by its first reconcile.
	eventually(t, "the secret to be flagged", 30*time.Second, func() bool { return len(reviewerComments(w, leaky.ID)) > 0 })
	eventually(t, "the clean change to be inspected", 30*time.Second, func() bool { _, ok := reviewRecord(o, clean.ID); return ok })

	got := reviewerComments(w, leaky.ID)
	if len(got) != 1 {
		t.Fatalf("comments on the leaky proposal: %+v", got)
	}
	c := got[0]
	if c.Decision != agwp.DecisionComment {
		t.Fatalf("decision = %q; reviewers only comment", c.Decision)
	}
	for _, want := range []string{"Possible secret in added lines", "GitHub token", "`deploy/settings.txt:2`", "revoke it now"} {
		if !strings.Contains(c.Body, want) {
			t.Errorf("the comment lacks %q:\n%s", want, c.Body)
		}
	}
	if strings.Contains(c.Body, fakeToken) || strings.Contains(c.Body, fakeToken[4:]) {
		t.Errorf("the comment repeats the secret:\n%s", c.Body)
	}
	if c.Head != leaky.Head {
		t.Errorf("review of head %s, proposal head is %s", c.Head, leaky.Head)
	}
	if rs := reviewerComments(w, clean.ID); len(rs) != 0 {
		t.Fatalf("clean, tested code got a review: %+v", rs)
	}
	if rec, _ := reviewRecord(o, clean.ID); len(rec.Findings) != 0 || rec.Summary.Files != 2 {
		t.Fatalf("the clean change's record = %+v", rec)
	}

	// Opened after the service started: found through the event feed.
	var many []agwp.FileChange
	for i := 0; i < 8; i++ {
		many = append(many, put(fmt.Sprintf("docs/page-%d.md", i), fmt.Sprintf("# Page %d\n", i)))
	}
	big, _ := w.Propose("ws-big", "Eight small pages", many...)
	var src strings.Builder
	src.WriteString("package pkg\n\n")
	for i := 0; i < 12; i++ {
		fmt.Fprintf(&src, "func F%d() int { return %d }\n", i, i)
	}
	risky, _ := w.Propose("ws-risky", "Tweak the pipeline and add helpers",
		put(".github/workflows/ci.yml", "name: ci\non: push\n"), put("pkg/helpers.go", src.String()))

	for name, p := range map[string]*agwp.Proposal{"oversized": big, "protected": risky} {
		p := p
		eventually(t, "the "+name+" change to be reviewed", 30*time.Second, func() bool { return len(reviewerComments(w, p.ID)) > 0 })
	}
	if b := reviewerComments(w, big.ID)[0].Body; !strings.Contains(b, "Large change") || !strings.Contains(b, "8 files") {
		t.Errorf("oversized change:\n%s", b)
	}
	b := reviewerComments(w, risky.ID)[0].Body
	for _, want := range []string{"Touches protected or sensitive paths", ".github/workflows/ci.yml", "Source changes without test changes", "pkg/helpers.go"} {
		if !strings.Contains(b, want) {
			t.Errorf("protected/untested change lacks %q:\n%s", want, b)
		}
	}
	if strings.Contains(b, "Possible secret") || strings.Contains(b, "Large change") {
		t.Errorf("findings that do not apply:\n%s", b)
	}

	// Nobody approved anything, the clean change is still unreviewed, and each comment was posted once.
	time.Sleep(2500 * time.Millisecond)
	for _, p := range []*agwp.Proposal{clean, leaky, big, risky} {
		d, err := w.Alice.GetProposal(ctx, w.Repo, p.ID)
		if err != nil {
			t.Fatal(err)
		}
		n := 0
		for _, r := range d.Reviews {
			if r.Reviewer == "service:reviewer" {
				n++
				if r.Decision != agwp.DecisionComment {
					t.Errorf("proposal %s: decision %q", p.ID, r.Decision)
				}
			}
		}
		want := 1
		if p.ID == clean.ID {
			want = 0
		}
		if n != want {
			t.Errorf("proposal %s has %d automated comments, want %d", p.ID, n, want)
		}
	}

	// The secret never appears in the orchestrator's own output either.
	_, status := o.get("/status")
	for name, text := range map[string]string{"log": o.logs.String(), "status": status} {
		if strings.Contains(text, fakeToken) {
			t.Errorf("the planted secret appears in the orchestrator's %s", name)
		}
	}
}
