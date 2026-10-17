//go:build linux

package integration

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/execsched"
)

func validationPolicy(ciKey string) string {
	return fmt.Sprintf(`{
  "trusted_issuers": [{"identity": "service:ci-1", "public_key": %q}],
  "required_validations": [{"check": "tests", "run": {"argv": ["true"], "timeout_secs": 60, "memory_mb": 128}}],
  "approvals": {"min": 1}
}`, ciKey)
}

func (o *orchestrator) proposalStatus(id string) (execsched.ProposalStatus, bool) {
	for _, r := range o.status().Repos {
		if r.Validation == nil {
			continue
		}
		for _, p := range r.Validation.Open {
			if p.ID == id {
				return p, true
			}
		}
	}
	return execsched.ProposalStatus{}, false
}

func (o *orchestrator) integratedStatus(id string) bool {
	for _, r := range o.status().Repos {
		if r.Validation == nil {
			continue
		}
		for _, p := range r.Validation.Integrated {
			if p.ID == id {
				return true
			}
		}
	}
	return false
}

func (w *world) jobsFor(proposal string) []agwp.Job {
	w.t.Helper()
	jobs, err := w.alice.ListJobs(context.Background(), w.repo, "", 100)
	if err != nil {
		w.t.Fatalf("list jobs: %v", err)
	}
	var out []agwp.Job
	for _, j := range jobs {
		if j.Validation != nil && j.Validation.Proposal == proposal {
			out = append(out, j)
		}
	}
	return out
}

// The execution scheduler keeps proposals prepared against the real server's
// execution plane: it asks for validation jobs (once per candidate), tracks the
// four states independently as approvals and attestations arrive from other
// parties, follows a moved head with a new job, and never lands anything.
func TestExecutionSchedulerTracksRealValidationJobs(t *testing.T) {
	w := newWorld(t, worldOptions{})
	ctx := context.Background()
	w.seed(map[string]string{"README.md": "# web\n"}) // before the policy: the seed needs no validation
	if err := w.srv.admin.SetPolicy(ctx, w.repo, "main", validationPolicy(w.ci.key.PublicHex())); err != nil {
		t.Fatal(err)
	}

	o := w.startOrchestrator(orchConfig{
		Repo: "    schedule: {enabled: false}\n    advisor: {enabled: false}\n    review: {enabled: false}\n    index: {enabled: false}\n",
	})
	o.waitReady()

	p1, _ := w.propose("ws-one", "Add page one", put("docs/one.md", "one\n"))

	// A job is queued for the exact candidate; validation is pending, approval is missing, the change merges.
	var st execsched.ProposalStatus
	eventually(t, "the proposal to be prepared with a queued job", 30*time.Second, func() bool {
		var ok bool
		st, ok = o.proposalStatus(p1.ID)
		return ok && len(st.Jobs) == 1
	})
	jobs := w.jobsFor(p1.ID)
	if len(jobs) != 1 || jobs[0].State != agwp.JobQueued || jobs[0].Validation.Check != "tests" || jobs[0].Validation.TreeID != st.CandidateTree {
		t.Fatalf("jobs = %+v, candidate tree %s", jobs, st.CandidateTree)
	}
	if st.Mergeable != execsched.Yes || st.Validated != execsched.Pending || st.Approved != execsched.No || st.Integrated || st.Ready {
		t.Fatalf("states = mergeable %s, validated %s, approved %s, integrated %v, ready %v", st.Mergeable, st.Validated, st.Approved, st.Integrated, st.Ready)
	}
	// Asking again and again does not queue more jobs.
	time.Sleep(2500 * time.Millisecond)
	if n := len(w.jobsFor(p1.ID)); n != 1 {
		t.Fatalf("%d jobs for one candidate", n)
	}

	// The four states move independently. First an approval (by a person) ...
	if _, err := w.bob.ReviewProposal(ctx, w.repo, p1.ID, agwp.DecisionApprove, "fine by me"); err != nil {
		t.Fatal(err)
	}
	eventually(t, "the approval to show", 30*time.Second, func() bool {
		s, ok := o.proposalStatus(p1.ID)
		return ok && s.Approved == execsched.Yes
	})
	if s, _ := o.proposalStatus(p1.ID); s.Validated != execsched.Pending || s.Ready {
		t.Fatalf("approved but not validated: %+v", s)
	}
	if res, err := w.carol.Integrate(ctx, w.repo, p1.ID, nil); err != nil || res.Outcome.Status != "blocked" {
		t.Fatalf("integrating before the validation ran: %+v %v", res, err)
	}

	// ... then the trusted worker claims the job and attests the exact tree.
	job := w.ci.claim()
	if job == nil || job.ID != jobs[0].ID {
		t.Fatalf("claimed %+v, want job %s", job, jobs[0].ID)
	}
	if again := w.ci.claim(); again != nil {
		t.Fatalf("a second claim got a job: %+v", again)
	}
	w.ci.complete(job, "pass")
	eventually(t, "validation to be passed and the proposal ready", 30*time.Second, func() bool {
		s, ok := o.proposalStatus(p1.ID)
		return ok && s.Validated == execsched.Yes && s.Ready
	})

	// The orchestrator never integrates: the proposal is still open until a person does it.
	time.Sleep(1500 * time.Millisecond)
	if d, _ := w.alice.GetProposal(ctx, w.repo, p1.ID); d.Proposal.State != agwp.ProposalOpen {
		t.Fatalf("proposal state = %s", d.Proposal.State)
	}
	w.land(p1)
	eventually(t, "the landed proposal to move to the recent list", 30*time.Second, func() bool { return o.integratedStatus(p1.ID) })
	if _, open := o.proposalStatus(p1.ID); open {
		t.Fatal("an integrated proposal is still listed as open")
	}

	// A failing attestation: validated = no, with the server's reason, and nothing can land.
	p2, ws2 := w.propose("ws-two", "Add page two", put("docs/two.md", "two\n"))
	eventually(t, "a job for the second proposal", 30*time.Second, func() bool { return len(w.jobsFor(p2.ID)) == 1 })
	failing := w.ci.claim()
	if failing == nil || failing.Validation.Proposal != p2.ID {
		t.Fatalf("claimed %+v", failing)
	}
	w.ci.complete(failing, "fail")
	// integration.prepare is not read-consistent around a job's completion (it computes the
	// preparation, then lists the jobs), so for a moment a response can show the validation as
	// missing while its job is already done. The scheduler re-prepares on the job and attestation
	// events and so converges; the test waits for the converged answer.
	var s2 execsched.ProposalStatus
	eventually(t, "the failed validation to show", 30*time.Second, func() bool {
		var ok bool
		s2, ok = o.proposalStatus(p2.ID)
		if !ok || s2.Validated != execsched.No {
			return false
		}
		for _, b := range s2.Blockers {
			if b.Code == agwp.BlockerFailedValidation {
				return true
			}
		}
		return false
	})
	if s2.Ready || len(s2.Validations) != 1 || s2.Validations[0].State != "failed" || s2.Validations[0].AttestationID == "" {
		t.Fatalf("a failed validation must block and name the attestation: %+v", s2)
	}
	if res, err := w.carol.Integrate(ctx, w.repo, p2.ID, nil); err != nil || res.Outcome.Status == "integrated" || !hasBlocker(res.Outcome.Blockers, agwp.BlockerFailedValidation) {
		t.Fatalf("the server must refuse a proposal with a failed validation: %+v %v", res, err)
	}

	// Moving the head makes a new candidate; the old verdict does not carry over and a new job is queued.
	moved, err := w.alice.ModifyWorkspace(ctx, w.repo, ws2.Name, "fix page two", "", []agwp.FileChange{put("docs/two.md", "two, fixed\n")})
	if err != nil {
		t.Fatal(err)
	}
	w.updateProposal(p2.ID, moved.Head)
	eventually(t, "a new job for the moved head", 30*time.Second, func() bool { return len(w.jobsFor(p2.ID)) == 2 })
	eventually(t, "validation to be pending again", 30*time.Second, func() bool {
		s, ok := o.proposalStatus(p2.ID)
		return ok && s.Head == moved.Head && s.Validated == execsched.Pending
	})
	trees := map[string]bool{}
	for _, j := range w.jobsFor(p2.ID) {
		trees[j.Validation.TreeID] = true
	}
	if len(trees) != 2 {
		t.Fatalf("the two jobs must be for two different candidate trees: %v", trees)
	}

	// Accounting: queued by the orchestrator, claimed and completed by the worker only.
	_, metrics := o.get("/metrics")
	if !strings.Contains(metrics, `orchestrator_validations_queued_total{repo="acme/web"}`) {
		t.Errorf("queued validations are not counted:\n%s", metrics)
	}
	events, _, err := w.alice.ReplayEvents(ctx, w.repo, 0, 1000, []string{"job"})
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range events {
		switch e.Topic {
		case "job.claim", "job.complete":
			if e.Payload.Actor != "service:ci-1" {
				t.Errorf("%s by %s; only the CI worker may run jobs", e.Topic, e.Payload.Actor)
			}
		case "job.enqueue":
			if e.Payload.Actor != "service:orchestrator" {
				t.Errorf("job.enqueue by %s", e.Payload.Actor)
			}
		}
	}
}
