package testutil

import (
	"context"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

// RepoName is the repository every World creates.
const RepoName = "acme/web"

// World is a real server with a repository, people and a CI worker.
type World struct {
	T      testing.TB
	Srv    *Server
	Repo   string
	Alice  *agwp.Client // writer: files tasks, prepares workspaces
	Bob    *agwp.Client // reviewer: approves
	Carol  *agwp.Client // integrator: the only one who lands changes
	CI     *CIWorker
	Orch   *agwp.Client // service:orchestrator (delegator), every operation its role allows
	AliceT string       // credentials
	BobT   string
	CarolT string
	OrchT  string
}

// WorldOptions configures NewWorld.
type WorldOptions struct {
	// Policy returns the policy document for the main stream, given the public
	// key (hex) of the CI worker the policy should trust. Nil: no policy.
	Policy func(ciPublicKey string) string
	// Agents are the agent names that get the writer role (and so can be given
	// credentials by the orchestrator).
	Agents []string
	// Server is an already started server to build the world on; nil: start one.
	Server *Server
}

// NewWorld starts a real server (unless o.Server is set) and creates the repository, the roles and a CI worker.
func NewWorld(t testing.TB, o WorldOptions) *World {
	t.Helper()
	srv := o.Server
	if srv == nil {
		srv = StartServer(t)
	}
	ctx := context.Background()
	w := &World{T: t, Srv: srv, Repo: RepoName}
	if _, err := srv.Admin.CreateRepo(ctx, RepoName); err != nil {
		t.Fatalf("create repo: %v", err)
	}
	grant := func(subject, role string) {
		t.Helper()
		if err := srv.Admin.GrantRole(ctx, subject, RepoName, role); err != nil {
			t.Fatalf("grant %s %s: %v", subject, role, err)
		}
	}
	for subject, role := range map[string]string{
		"user:alice": "writer", "user:bob": "reviewer", "user:carol": "integrator",
		"service:orchestrator": "delegator", "service:reviewer": "reviewer", "service:advisor": "reviewer", "service:ci-1": "runner",
	} {
		grant(subject, role)
	}
	for _, a := range o.Agents {
		grant("agent:"+a, "writer")
	}
	w.AliceT, w.BobT, w.CarolT = w.Mint("user:alice", nil), w.Mint("user:bob", nil), w.Mint("user:carol", nil)
	w.Alice, w.Bob, w.Carol = srv.Client(w.AliceT, "alice"), srv.Client(w.BobT, "bob"), srv.Client(w.CarolT, "carol")
	w.OrchT = w.Mint("service:orchestrator", nil)
	w.Orch = srv.Client(w.OrchT, "orchestrator")

	key, err := agentkey.Generate()
	if err != nil {
		t.Fatal(err)
	}
	w.CI = &CIWorker{w: w, issuer: "service:ci-1", key: key, token: w.Mint("service:ci-1", nil)}
	if o.Policy != nil {
		if err := srv.Admin.SetPolicy(ctx, RepoName, "main", o.Policy(key.PublicHex())); err != nil {
			t.Fatalf("set policy: %v", err)
		}
	}
	return w
}

// Mint asks the server for a credential of subject (as an administrator). ops
// narrows the operations (nil: everything the subject's role allows).
func (w *World) Mint(subject string, ops []string) string {
	w.T.Helper()
	ttl := uint32(7 * 24 * 3600)
	if strings.HasPrefix(subject, "agent:") {
		ttl = 3600 // the server caps agent credentials at an hour
	}
	m, err := w.Srv.Admin.MintCredential(context.Background(), agwp.MintRequest{
		Subject: subject, TTLSecs: ttl, Description: "test", Scope: agwp.CredentialScope{Operations: ops},
	})
	if err != nil {
		w.T.Fatalf("mint credential for %s: %v", subject, err)
	}
	return m.Token.Reveal()
}

// Put is a file-put change.
func Put(path, content string) agwp.FileChange {
	return agwp.FileChange{Path: path, Op: "put", Content: []byte(content)}
}

// Delete is a file-delete change.
func Delete(path string) agwp.FileChange { return agwp.FileChange{Path: path, Op: "delete"} }

// Propose opens a proposal against main from a fresh workspace, as alice.
func (w *World) Propose(ws, title string, changes ...agwp.FileChange) (*agwp.Proposal, *agwp.Workspace) {
	w.T.Helper()
	return w.ProposeOn("main", ws, title, changes...)
}

// ProposeOn is Propose against another stream.
func (w *World) ProposeOn(stream, ws, title string, changes ...agwp.FileChange) (*agwp.Proposal, *agwp.Workspace) {
	w.T.Helper()
	ctx := context.Background()
	if _, err := w.Alice.CreateWorkspace(ctx, w.Repo, ws, stream); err != nil {
		w.T.Fatalf("create workspace %s: %v", ws, err)
	}
	wsp, err := w.Alice.ModifyWorkspace(ctx, w.Repo, ws, title, "", changes)
	if err != nil {
		w.T.Fatalf("modify workspace %s: %v", ws, err)
	}
	p, err := w.Alice.CreateProposal(ctx, w.Repo, agwp.ProposalInput{Stream: stream, Head: wsp.Head, Title: title})
	if err != nil {
		w.T.Fatalf("open proposal %q: %v", title, err)
	}
	return p, wsp
}

// UpdateProposal moves a proposal to a new head (what a person does after
// resolving conflicts in a workspace).
func (w *World) UpdateProposal(id, head string) {
	w.T.Helper()
	code, body := w.Srv.Call(w.AliceT, "PATCH", "/v1/repos/"+w.Repo+"/proposals/"+id, map[string]any{"new_head": head})
	if code != 200 {
		w.T.Fatalf("update proposal %s: %d %s", id, code, body)
	}
}

// Land integrates a proposal as carol and requires that it worked.
func (w *World) Land(p *agwp.Proposal) {
	w.T.Helper()
	res, err := w.Carol.Integrate(context.Background(), w.Repo, p.ID, nil)
	if err != nil {
		w.T.Fatalf("integrate %s: %v", p.ID, err)
	}
	if res.Outcome.Status != "integrated" {
		w.T.Fatalf("proposal %s did not integrate: %+v", p.ID, res.Outcome)
	}
}

// Seed lands a change made by a human and returns the new head of main.
func (w *World) Seed(files map[string]string) string {
	w.T.Helper()
	var ch []agwp.FileChange
	for p, c := range files {
		ch = append(ch, Put(p, c))
	}
	p, _ := w.Propose("seed-"+RandHex(3), "Seed", ch...)
	w.Land(p)
	return w.Head("main")
}

// Head is the head changeset of a stream.
func (w *World) Head(stream string) string {
	w.T.Helper()
	ss, err := w.Alice.ListStreams(context.Background(), w.Repo)
	if err != nil {
		w.T.Fatalf("list streams: %v", err)
	}
	for _, s := range ss {
		if s.Name == stream {
			return s.Head
		}
	}
	w.T.Fatalf("stream %q does not exist", stream)
	return ""
}

// Task files a task as alice.
func (w *World) Task(title, desc string, scope []string, deps ...string) *agwp.Task {
	w.T.Helper()
	tk, err := w.Alice.CreateTask(context.Background(), w.Repo, title, desc, scope, deps)
	if err != nil {
		w.T.Fatalf("create task %q: %v", title, err)
	}
	return tk
}

// GetTask reads a task.
func (w *World) GetTask(id string) *agwp.Task {
	w.T.Helper()
	tk, err := w.Alice.GetTask(context.Background(), w.Repo, id)
	if err != nil {
		w.T.Fatalf("get task %s: %v", id, err)
	}
	return tk
}

// ProposalsOfTask lists the proposals that belong to a task.
func (w *World) ProposalsOfTask(id string) []agwp.Proposal {
	w.T.Helper()
	all, err := w.Alice.ListProposals(context.Background(), w.Repo, agwp.ProposalFilter{})
	if err != nil {
		w.T.Fatalf("list proposals: %v", err)
	}
	var out []agwp.Proposal
	for _, p := range all {
		if p.TaskID == id {
			out = append(out, p)
		}
	}
	return out
}

// File reads a file at a revision; false when it does not exist there.
func (w *World) File(rev, path string) (string, bool) {
	w.T.Helper()
	b, err := w.Alice.GetFile(context.Background(), w.Repo, rev, path, 1<<20)
	if err != nil {
		if agwp.IsNotFound(err) {
			return "", false
		}
		w.T.Fatalf("read %s@%s: %v", path, rev, err)
	}
	return string(b), true
}

// Approve records bob's approval of the proposal's current head.
func (w *World) Approve(id string) {
	w.T.Helper()
	if _, err := w.Bob.ReviewProposal(context.Background(), w.Repo, id, agwp.DecisionApprove, "approved in a test"); err != nil {
		w.T.Fatalf("approve %s: %v", id, err)
	}
}
