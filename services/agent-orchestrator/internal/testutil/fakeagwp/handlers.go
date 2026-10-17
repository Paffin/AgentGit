package fakeagwp

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

// handle runs one operation with s.mu held.
func (s *Server) handle(c *reqCtx) (any, int, *apiErr) {
	r := c.repo
	switch c.op {
	case "repo.describe":
		return agwp.Repo{Name: r.name, Namespace: r.name, DefaultStream: "main", StreamCount: uint32(len(r.streams))}, 0, nil
	case "stream.list":
		var out []agwp.Stream
		for name, st := range r.streams {
			out = append(out, agwp.Stream{Name: name, Head: st.head, Version: st.version})
		}
		sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
		return map[string]any{"streams": out}, 0, nil
	case "stream.head":
		name := c.q.Get("stream")
		st := r.streams[name]
		if st == nil {
			return nil, 0, errNotFound("stream '" + name + "'")
		}
		return agwp.Stream{Name: name, Head: st.head, Version: st.version}, 0, nil

	case "agent.list":
		var out []agwp.Agent
		for _, a := range r.agents {
			out = append(out, *a)
		}
		sort.Slice(out, func(i, j int) bool { return out[i].Name < out[j].Name })
		if out == nil {
			out = []agwp.Agent{}
		}
		return map[string]any{"agents": out}, 0, nil
	case "agent.register":
		var in struct{ Name, PublicKey, Role string }
		var raw map[string]string
		if e := c.decode(&raw); e != nil {
			return nil, 0, e
		}
		in.Name, in.PublicKey = raw["name"], raw["public_key"]
		if in.Name == "" || len(in.PublicKey) != 64 {
			return nil, 0, errInvalid("name and a 64-hex public_key are required")
		}
		if kindOf(c.tok.identity) == "agent" && c.tok.identity != "agent:"+in.Name {
			return nil, 0, errDenied("%s may not act for agent '%s'", c.tok.identity, in.Name)
		}
		if _, dup := r.agents[in.Name]; dup {
			return nil, 0, errf(409, "already_exists", "agent '%s' already exists", in.Name)
		}
		a := &agwp.Agent{Name: in.Name, Identity: "agent:" + in.Name, PublicKey: in.PublicKey, Role: "agent"}
		r.agents[in.Name] = a
		s.emitLocked(r, "agent.register", c.identity, a.Identity, nil)
		return a, 0, nil

	case "agent.start_run":
		var in map[string]string
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		agent := in["agent"]
		a := r.agents[agent]
		if a == nil {
			return nil, 0, errNotFound("agent '" + agent + "'")
		}
		if kindOf(c.tok.identity) == "agent" && c.tok.identity != a.Identity {
			return nil, 0, errDenied("%s may not act for agent '%s'", c.tok.identity, agent)
		}
		if tid := in["task_id"]; tid != "" {
			t := s.taskLocked(r, tid)
			if t == nil {
				return nil, 0, errNotFound("task '" + tid + "'")
			}
			if t.State == agwp.TaskDone || t.State == agwp.TaskCancelled {
				return nil, 0, errf(412, "failed_precondition", "task '%s' is %s", tid, t.State)
			}
			if t.Owner != "" && t.Owner != a.Identity {
				return nil, 0, errf(412, "failed_precondition", "task '%s' is owned by %s", tid, t.Owner)
			}
			if !s.viewTaskLocked(r, t).Ready {
				return nil, 0, errf(412, "failed_precondition", "task '%s' is blocked by unfinished tasks", tid)
			}
			t.Owner, t.State, t.UpdatedAtMs = a.Identity, agwp.TaskInProgress, s.nowMs()
		}
		run := &agwp.Run{RunID: "run_" + randHex(8), Agent: agent, TaskID: in["task_id"], Status: "running", StartedAtMs: s.nowMs(), LastHeartbeatMs: s.nowMs()}
		run.Identity = a.Identity + "#" + run.RunID
		r.runs = append(r.runs, run)
		s.emitLocked(r, "run.start", run.Identity, run.RunID, map[string]any{"task": in["task_id"]})
		return run, 0, nil
	case "agent.heartbeat", "agent.complete":
		run := s.runLocked(r, c.rest[1])
		if run == nil {
			return nil, 0, errNotFound("run '" + c.rest[1] + "'")
		}
		if kindOf(c.tok.identity) == "agent" && c.tok.identity != "agent:"+run.Agent {
			return nil, 0, errDenied("%s may not act for agent '%s'", c.tok.identity, run.Agent)
		}
		if run.Status != "running" {
			return nil, 0, errNotFound("running run '" + run.RunID + "'")
		}
		if c.op == "agent.heartbeat" {
			run.LastHeartbeatMs = s.nowMs()
			return map[string]any{"server_time_ms": s.nowMs()}, 0, nil
		}
		var in map[string]string
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if in["status"] != "completed" && in["status"] != "failed" {
			return nil, 0, errInvalid("status must be 'completed' or 'failed'")
		}
		run.Status, run.Summary, run.FinishedAtMs = in["status"], in["summary"], s.nowMs()
		for _, l := range r.leases {
			if l.Holder == run.Identity && l.ReleasedAtMs == 0 {
				l.ReleasedAtMs = s.nowMs()
			}
		}
		s.emitLocked(r, "run.finish", run.Identity, run.RunID, map[string]any{"status": run.Status})
		return run, 0, nil
	case "agent.list_runs":
		var out []agwp.Run
		for i := len(r.runs) - 1; i >= 0; i-- {
			ru := r.runs[i]
			if a := c.q.Get("agent"); a != "" && ru.Agent != a {
				continue
			}
			if c.q.Get("active_only") == "true" && ru.Status != "running" {
				continue
			}
			out = append(out, *ru)
		}
		if lim, _ := strconv.Atoi(c.q.Get("limit")); lim > 0 && len(out) > lim {
			out = out[:lim]
		}
		if out == nil {
			out = []agwp.Run{}
		}
		return map[string]any{"runs": out}, 0, nil

	case "task.list":
		out := []agwp.Task{}
		for _, t := range r.tasks {
			v := s.viewTaskLocked(r, t)
			if st := c.q.Get("state"); st != "" && v.State != st {
				continue
			}
			if c.q.Get("ready_only") == "true" && !(v.Ready && v.State == agwp.TaskOpen) {
				continue
			}
			out = append(out, v)
		}
		return map[string]any{"tasks": out}, 0, nil
	case "task.get":
		t := s.taskLocked(r, c.rest[1])
		if t == nil {
			return nil, 0, errNotFound("task '" + c.rest[1] + "'")
		}
		return s.viewTaskLocked(r, t), 0, nil
	case "task.create":
		var in struct {
			Title       string   `json:"title"`
			Description string   `json:"description"`
			Scope       []string `json:"scope"`
			DependsOn   []string `json:"depends_on"`
		}
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		t := &taskState{Task: agwp.Task{ID: "task_" + randHex(6), Title: in.Title, Description: in.Description, State: agwp.TaskOpen,
			Scope: in.Scope, DependsOn: in.DependsOn, CreatedBy: c.identity, CreatedAtMs: s.nowMs(), UpdatedAtMs: s.nowMs()}}
		r.tasks = append(r.tasks, t)
		s.emitLocked(r, "task.create", c.identity, t.ID, map[string]any{"title": in.Title})
		return s.viewTaskLocked(r, t), 0, nil
	case "task.finish":
		t := s.taskLocked(r, c.rest[1])
		if t == nil {
			return nil, 0, errNotFound("task '" + c.rest[1] + "'")
		}
		var in map[string]string
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if in["state"] != "done" && in["state"] != "cancelled" {
			return nil, 0, errInvalid("state must be 'done' or 'cancelled'")
		}
		if t.Owner != "" && t.Owner != principalOf(c.identity) {
			return nil, 0, errf(412, "failed_precondition", "task '%s' is owned by %s", t.ID, t.Owner)
		}
		t.State, t.UpdatedAtMs = in["state"], s.nowMs()
		s.emitLocked(r, "task.finish", c.identity, t.ID, map[string]any{"state": t.State})
		return s.viewTaskLocked(r, t), 0, nil

	case "lease.list":
		out := []agwp.Lease{}
		for _, l := range r.leases {
			if c.q.Get("active_only") == "true" && !l.Active(s.now()) {
				continue
			}
			out = append(out, *l)
		}
		return map[string]any{"leases": out}, 0, nil
	case "lease.acquire":
		var in struct {
			Scope     string `json:"scope"`
			Exclusive bool   `json:"exclusive"`
			TTLMs     int64  `json:"ttl_ms"`
			TaskID    string `json:"task_id"`
		}
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if in.Scope == "" || in.TTLMs < 1 || in.TTLMs > 24*3600*1000 {
			return nil, 0, errInvalid("scope and a ttl between 1 ms and 24 h are required")
		}
		var overlaps []agwp.Lease
		for _, l := range r.leases {
			if !l.Active(s.now()) || principalOf(l.Holder) == principalOf(c.identity) || !mayOverlap(l.Scope, in.Scope) {
				continue
			}
			if in.Exclusive && l.Exclusive {
				return nil, 0, errf(409, "lease_conflict", "scope '%s' overlaps an exclusive lease on '%s' held by %s", in.Scope, l.Scope, l.Holder)
			}
			overlaps = append(overlaps, *l)
		}
		r.fence++
		l := &agwp.Lease{ID: "lease_" + randHex(6), Scope: in.Scope, Holder: c.identity, Exclusive: in.Exclusive, FencingToken: r.fence,
			AcquiredAtMs: s.nowMs(), ExpiresAtMs: s.now().UnixMilli() + in.TTLMs, TaskID: in.TaskID}
		r.leases = append(r.leases, l)
		s.emitLocked(r, "lease.acquire", c.identity, l.ID, map[string]any{"scope": in.Scope, "exclusive": in.Exclusive, "token": l.FencingToken})
		if overlaps == nil {
			overlaps = []agwp.Lease{}
		}
		return agwp.AcquireLeaseResponse{Lease: *l, Overlapping: overlaps}, 0, nil
	case "lease.release", "lease.extend":
		var l *agwp.Lease
		for _, x := range r.leases {
			if x.ID == c.rest[1] {
				l = x
			}
		}
		if l == nil {
			return nil, 0, errNotFound("lease '" + c.rest[1] + "'")
		}
		if principalOf(l.Holder) != principalOf(c.identity) {
			return nil, 0, errf(412, "failed_precondition", "lease '%s' is held by %s", l.ID, l.Holder)
		}
		if c.op == "lease.release" {
			l.ReleasedAtMs = s.nowMs()
			s.emitLocked(r, "lease.release", c.identity, l.ID, nil)
			return map[string]any{}, 0, nil
		}
		var in struct {
			TTLMs int64 `json:"ttl_ms"`
		}
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		l.ExpiresAtMs = s.nowMs() + in.TTLMs
		return l, 0, nil
	case "agent.predict":
		out := r.predict
		if out == nil {
			out = []agwp.Overlap{}
		}
		return map[string]any{"overlaps": out}, 0, nil

	case "admin.credentials.mint":
		return s.mint(c)
	case "admin.credentials.revoke":
		t := s.creds[c.rest[0]]
		if t == nil {
			return nil, 0, errNotFound("credential")
		}
		if t.parent != c.tok.id && c.tok.role != "admin" {
			return nil, 0, errNotFound("credential")
		}
		t.revoked = true
		return map[string]any{"revoked": true}, 0, nil

	case "workspace.create":
		var in map[string]string
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		st := r.streams[in["stream"]]
		if st == nil || in["name"] == "" {
			return nil, 0, errInvalid("name and an existing stream are required")
		}
		if _, dup := r.workspaces[in["name"]]; dup {
			return nil, 0, errf(409, "already_exists", "workspace '%s' exists", in["name"])
		}
		w := &wsState{Workspace: agwp.Workspace{Name: in["name"], Stream: in["stream"], Base: st.head, Head: st.head, Owner: c.identity}}
		r.workspaces[w.Name] = w
		s.emitLocked(r, "workspace.create", c.identity, w.Name, map[string]any{"stream": w.Stream})
		return w.Workspace, 0, nil
	case "workspace.get":
		w := r.workspaces[c.rest[1]]
		if w == nil {
			return nil, 0, errNotFound("workspace '" + c.rest[1] + "'")
		}
		return w.Workspace, 0, nil
	case "workspace.delete":
		delete(r.workspaces, c.rest[1])
		return map[string]any{}, 0, nil
	case "workspace.modify":
		w := r.workspaces[c.rest[1]]
		if w == nil {
			return nil, 0, errNotFound("workspace '" + c.rest[1] + "'")
		}
		if principalOf(w.Owner) != principalOf(c.identity) {
			return nil, 0, errDenied("only %s may change workspace '%s'", w.Owner, w.Name)
		}
		var in struct {
			Message      string `json:"message"`
			ExpectedHead string `json:"expected_head"`
			Changes      []struct {
				Path    string `json:"path"`
				Op      string `json:"op"`
				Content string `json:"content"`
			} `json:"changes"`
		}
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if in.ExpectedHead != "" && in.ExpectedHead != w.Head {
			return nil, 0, errPrecond("workspace head is %s, expected %s", w.Head, in.ExpectedHead)
		}
		files := map[string]string{}
		if cs := r.changesets[w.Head]; cs != nil {
			for k, v := range cs.files {
				files[k] = string(v)
			}
		}
		for _, ch := range in.Changes {
			switch ch.Op {
			case "put":
				b, err := base64.StdEncoding.DecodeString(ch.Content)
				if err != nil {
					return nil, 0, errInvalid("content of %s is not base64", ch.Path)
				}
				files[ch.Path] = string(b)
			case "delete":
				delete(files, ch.Path)
			default:
				return nil, 0, errInvalid("op must be put or delete")
			}
		}
		var parents []string
		if w.Head != "" {
			parents = []string{w.Head}
		}
		w.Head = s.commitLocked(r, parents, files, c.identity, in.Message)
		s.emitLocked(r, "workspace.modify", c.identity, w.Name, map[string]any{"changeset": w.Head})
		return w.Workspace, 0, nil
	case "workspace.sync":
		w := r.workspaces[c.rest[1]]
		if w == nil {
			return nil, 0, errNotFound("workspace '" + c.rest[1] + "'")
		}
		return agwp.SyncResult{Status: "up_to_date", Head: w.Head, Conflicts: []agwp.Conflict{}}, 0, nil

	case "proposal.list":
		out := []agwp.Proposal{}
		for i := len(r.proposals) - 1; i >= 0; i-- {
			p := r.proposals[i]
			if v := c.q.Get("state"); v != "" && p.State != v {
				continue
			}
			if v := c.q.Get("stream"); v != "" && p.Stream != v {
				continue
			}
			if v := c.q.Get("author"); v != "" && p.Author != v {
				continue
			}
			out = append(out, *p)
		}
		return map[string]any{"proposals": out, "next_page_token": ""}, 0, nil
	case "proposal.create":
		var in agwp.ProposalInput
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if r.streams[in.Stream] == nil || r.changesets[in.Head] == nil {
			return nil, 0, errInvalid("unknown stream or head")
		}
		p := s.addProposalLocked(r, c.identity, in.Stream, in.Head, in.Title, in.TaskID)
		return p, 0, nil
	case "proposal.get":
		p := s.proposalLocked(r, c.rest[1])
		if p == nil {
			return nil, 0, errNotFound("proposal '" + c.rest[1] + "'")
		}
		revs := r.reviews[p.ID]
		if revs == nil {
			revs = []agwp.Review{}
		}
		return agwp.ProposalDetail{Proposal: *p, Reviews: revs}, 0, nil
	case "proposal.close":
		p := s.proposalLocked(r, c.rest[1])
		if p == nil {
			return nil, 0, errNotFound("proposal '" + c.rest[1] + "'")
		}
		if p.State != agwp.ProposalOpen {
			return nil, 0, errf(409, "aborted", "proposal is already %s", p.State)
		}
		p.State = agwp.ProposalClosed
		s.emitLocked(r, "proposal.close", c.identity, p.ID, nil)
		return p, 0, nil
	case "proposal.review":
		p := s.proposalLocked(r, c.rest[1])
		if p == nil {
			return nil, 0, errNotFound("proposal '" + c.rest[1] + "'")
		}
		var in map[string]string
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if d := in["decision"]; d != "approve" && d != "request_changes" && d != "comment" {
			return nil, 0, errInvalid("unknown decision '%s'", d)
		}
		if p.State != agwp.ProposalOpen {
			return nil, 0, errPrecond("proposal '%s' is %s", p.ID, p.State)
		}
		r.reviewSeq++
		rv := agwp.Review{ID: r.reviewSeq, Proposal: p.ID, Reviewer: c.identity, Decision: in["decision"], Body: in["body"], Head: p.Head, TimestampMs: s.nowMs()}
		r.reviews[p.ID] = append(r.reviews[p.ID], rv)
		if rv.Decision == "approve" {
			k := p.ID + "@" + p.Head
			if r.approvals[k] == nil {
				r.approvals[k] = map[string]bool{}
			}
			r.approvals[k][principalOf(c.identity)] = true
		}
		s.emitLocked(r, "proposal.review", c.identity, p.ID, map[string]any{"decision": rv.Decision, "head": p.Head})
		return rv, 0, nil

	case "integration.prepare":
		p := s.proposalLocked(r, c.rest[1])
		if p == nil {
			return nil, 0, errNotFound("proposal '" + c.rest[1] + "'")
		}
		var in struct {
			Enqueue bool `json:"enqueue_validations"`
		}
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if in.Enqueue && !roleAllows(c.tok.role, "integrate.validate") {
			return nil, 0, errDenied("integrate.validate required")
		}
		prep := s.prepareLocked(r, p)
		if in.Enqueue && s.StubJobs {
			for i, v := range prep.Validations {
				if v.State == "missing" {
					j := agwp.Job{ID: "job_" + randHex(4), Kind: "validation", State: "queued", Repo: r.name}
					r.jobs = append(r.jobs, j)
					prep.Jobs = append(prep.Jobs, j)
					prep.Validations[i].State, prep.Validations[i].JobID = "pending", j.ID
				}
			}
		}
		return prep, 0, nil
	case "integration.integrate":
		p := s.proposalLocked(r, c.rest[1])
		if p == nil {
			return nil, 0, errNotFound("proposal '" + c.rest[1] + "'")
		}
		prep := s.prepareLocked(r, p)
		if len(prep.Blockers) > 0 || len(prep.Conflicts) > 0 {
			status := "blocked"
			if len(prep.Conflicts) > 0 {
				status = "conflict"
			}
			return agwp.IntegrationResult{OperationID: "op_" + randHex(4), State: status,
				Outcome: agwp.IntegrationOutcome{Status: status, Blockers: prep.Blockers, Conflicts: prep.Conflicts}}, 0, nil
		}
		p.State, p.IntegratedAs = agwp.ProposalIntegrated, p.Head
		st := r.streams[p.Stream]
		prev := st.head
		st.head = p.Head
		st.version++
		s.emitLocked(r, "stream.integrate", c.identity, p.Stream, map[string]any{"new_head": p.Head, "old_head": prev, "proposal": p.ID, "version": st.version})
		return agwp.IntegrationResult{OperationID: "op_" + randHex(4), State: "integrated",
			Outcome: agwp.IntegrationOutcome{Status: "integrated", NewHead: p.Head, PreviousHead: prev, Version: st.version}}, 0, nil
	case "admin.policy.set":
		var in map[string]string
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		return map[string]any{"stream": in["stream"], "version": 1}, 0, nil
	case "queue.list":
		return map[string]any{"entries": []agwp.QueueEntry{}}, 0, nil
	case "jobs.list":
		return map[string]any{"jobs": append([]agwp.Job{}, r.jobs...)}, 0, nil

	case "tree.get":
		cs := s.resolveLocked(r, c.q.Get("rev"))
		if cs == nil {
			return nil, 0, errNotFound("revision '" + c.q.Get("rev") + "'")
		}
		paths := make([]string, 0, len(cs.files))
		for p := range cs.files {
			paths = append(paths, p)
		}
		sort.Strings(paths)
		entries := []agwp.TreeEntry{}
		for _, p := range paths {
			entries = append(entries, agwp.TreeEntry{Path: p, BlobID: fmt.Sprintf("blob-%x", len(cs.files[p])), Size: uint64(len(cs.files[p])), FileType: "file"})
		}
		return map[string]any{"tree_id": "tree-" + cs.id[:12], "entries": entries, "next_page_token": ""}, 0, nil
	case "tree.file":
		cs := s.resolveLocked(r, c.q.Get("rev"))
		if cs == nil {
			return nil, 0, errNotFound("revision '" + c.q.Get("rev") + "'")
		}
		b, ok := cs.files[strings.Join(c.rest[1:], "/")]
		if !ok {
			return nil, 0, errNotFound("path '" + strings.Join(c.rest[1:], "/") + "'")
		}
		return b, 0, nil
	case "history.resolve":
		cs := s.resolveLocked(r, c.q.Get("rev"))
		if cs == nil {
			return nil, 0, errNotFound("revision '" + c.q.Get("rev") + "'")
		}
		return map[string]any{"changeset_id": cs.id}, 0, nil

	case "provenance.submit":
		var in struct {
			Record    agwp.ProvenanceRecord `json:"record"`
			Signature string                `json:"signature"`
		}
		if e := c.decode(&in); e != nil {
			return nil, 0, e
		}
		if principalOf(in.Record.Agent) != principalOf(c.identity) {
			return nil, 0, errDenied("this credential authenticates as %s; it may not submit provenance for %s", c.identity, in.Record.Agent)
		}
		a := r.agents[strings.TrimPrefix(principalOf(in.Record.Agent), "agent:")]
		if a == nil {
			return nil, 0, errNotFound("agent " + in.Record.Agent)
		}
		if len(in.Signature) != 128 {
			return nil, 0, errInvalid("signature must be 128 hex characters")
		}
		e := agwp.ProvenanceEntry{ID: int64(len(r.provenance) + 1), Record: in.Record, PublicKey: a.PublicKey, Signature: in.Signature, Verified: true}
		r.provenance = append(r.provenance, e)
		s.emitLocked(r, "provenance.attach", c.identity, in.Record.Changeset, nil)
		return e, 0, nil
	case "provenance.list":
		out := []agwp.ProvenanceEntry{}
		for _, e := range r.provenance {
			if e.Record.Changeset == c.q.Get("changeset") {
				out = append(out, e)
			}
		}
		return map[string]any{"entries": out}, 0, nil

	case "events.replay":
		after, _ := strconv.ParseUint(c.q.Get("after_seq"), 10, 64)
		limit, _ := strconv.Atoi(c.q.Get("limit"))
		if limit <= 0 {
			limit = 500
		}
		topics := splitTopics(c.q.Get("topics"))
		out := []agwp.Event{}
		for _, e := range r.events {
			if e.Seq > after && topicMatch(topics, e.Topic) && len(out) < limit {
				out = append(out, e)
			}
		}
		return map[string]any{"events": out, "latest_seq": uint64(len(r.events))}, 0, nil
	}
	return nil, 0, errf(501, "unimplemented", "the fake does not implement %s", c.op)
}

func (s *Server) runLocked(r *repoState, id string) *agwp.Run {
	for _, ru := range r.runs {
		if ru.RunID == id {
			return ru
		}
	}
	return nil
}

func (s *Server) proposalLocked(r *repoState, id string) *agwp.Proposal {
	for _, p := range r.proposals {
		if p.ID == id {
			return p
		}
	}
	return nil
}

func mayOverlap(a, b string) bool {
	pa, pb := literalPrefix(a), literalPrefix(b)
	return strings.HasPrefix(pa, pb) || strings.HasPrefix(pb, pa)
}

func literalPrefix(g string) string {
	if i := strings.IndexAny(g, "*?[{"); i >= 0 {
		return g[:i]
	}
	return g
}

func splitTopics(s string) []string {
	var out []string
	for _, t := range strings.Split(s, ",") {
		if t != "" {
			out = append(out, t)
		}
	}
	return out
}

func topicMatch(topics []string, topic string) bool {
	if len(topics) == 0 {
		return true
	}
	for _, t := range topics {
		if t == "*" || topic == t || strings.HasPrefix(topic, t+".") {
			return true
		}
	}
	return false
}

// prepareLocked computes (or returns the scripted) preparation of a proposal.
func (s *Server) prepareLocked(r *repoState, p *agwp.Proposal) agwp.Preparation {
	if sp, ok := r.prepares[p.ID]; ok {
		cp := *sp
		cp.Proposal = p.ID
		cp.Validations = append([]agwp.ValidationStatus(nil), sp.Validations...)
		cp.Blockers = append([]agwp.Blocker(nil), sp.Blockers...)
		cp.Jobs = append([]agwp.Job(nil), sp.Jobs...)
		return nonNilPrep(cp)
	}
	st := r.streams[p.Stream]
	head := r.changesets[p.Head]
	prep := agwp.Preparation{Proposal: p.ID, Strategy: "fast_forward", Algorithm: "ag-merge/1+ag-diff/1", PolicyVersion: 1}
	var cur map[string][]byte
	if cs := r.changesets[st.head]; cs != nil {
		cur = cs.files
	}
	if head != nil {
		seen := map[string]bool{}
		for path, b := range head.files {
			seen[path] = true
			if old, ok := cur[path]; !ok || string(old) != string(b) {
				prep.ChangedFiles = append(prep.ChangedFiles, path)
			}
		}
		for path := range cur {
			if !seen[path] {
				prep.ChangedFiles = append(prep.ChangedFiles, path)
			}
		}
		sort.Strings(prep.ChangedFiles)
		prep.CandidateTree = "tree-" + head.id[:12]
	}
	pol := r.policy[p.Stream]
	for _, chk := range pol.RequiredChecks {
		state := "missing"
		if r.attested[prep.CandidateTree][chk] {
			state = "passed"
		} else {
			prep.Blockers = append(prep.Blockers, agwp.Blocker{Code: agwp.BlockerMissingValidation, Message: "check '" + chk + "' has no trusted passing attestation for this exact tree"})
		}
		prep.Validations = append(prep.Validations, agwp.ValidationStatus{Check: chk, State: state})
	}
	if have := len(r.approvals[p.ID+"@"+p.Head]); pol.MinApprovals > have {
		prep.Blockers = append(prep.Blockers, agwp.Blocker{Code: agwp.BlockerInsufficientApproval, Message: fmt.Sprintf("approvals (default): %d of %d required", have, pol.MinApprovals)})
	}
	return nonNilPrep(prep)
}

func nonNilPrep(p agwp.Preparation) agwp.Preparation {
	if p.Conflicts == nil {
		p.Conflicts = []agwp.Conflict{}
	}
	if p.Blockers == nil {
		p.Blockers = []agwp.Blocker{}
	}
	if p.Validations == nil {
		p.Validations = []agwp.ValidationStatus{}
	}
	if p.Jobs == nil {
		p.Jobs = []agwp.Job{}
	}
	if p.ChangedFiles == nil {
		p.ChangedFiles = []string{}
	}
	return p
}

// mint implements POST /v1/credentials with the real server's containment rule.
func (s *Server) mint(c *reqCtx) (any, int, *apiErr) {
	var in struct {
		Subject     string               `json:"subject"`
		Description string               `json:"description"`
		TTLSecs     uint32               `json:"ttl_secs"`
		Scope       agwp.CredentialScope `json:"scope"`
	}
	if e := c.decode(&in); e != nil {
		return nil, 0, e
	}
	subject := in.Subject
	if subject == "" {
		subject = principalOf(c.identity)
	}
	if subject != principalOf(c.identity) && c.tok.role != "admin" {
		return nil, 0, errDenied("minting credentials for %s requires an administrator", subject)
	}
	// scope containment: a child may never exceed its parent
	if len(c.tok.ops) > 0 {
		if len(in.Scope.Operations) == 0 {
			return nil, 0, errDenied("a credential cannot grant more than the credential used to mint it")
		}
		for _, o := range in.Scope.Operations {
			if !c.tok.ops[o] {
				return nil, 0, errDenied("a credential cannot grant more than the credential used to mint it (%s)", o)
			}
		}
	}
	if len(c.tok.repos) > 0 {
		if len(in.Scope.Repos) == 0 {
			return nil, 0, errDenied("a credential cannot grant more than the credential used to mint it")
		}
		for _, rp := range in.Scope.Repos {
			ok := false
			for _, parent := range c.tok.repos {
				if matchSimple(parent, rp) {
					ok = true
				}
			}
			if !ok {
				return nil, 0, errDenied("a credential cannot grant more than the credential used to mint it (%s)", rp)
			}
		}
	}
	ttl := time.Duration(in.TTLSecs) * time.Second
	if kindOf(subject) == "agent" {
		if ttl == 0 {
			ttl = time.Hour
		}
		if ttl > time.Hour {
			return nil, 0, errInvalid("ttl_secs may be at most 3600 for agent credentials")
		}
	}
	if ttl == 0 {
		ttl = 24 * time.Hour
	}
	if !c.tok.expiresAt.IsZero() && s.now().Add(ttl).After(c.tok.expiresAt) {
		ttl = c.tok.expiresAt.Sub(s.now())
	}
	role := "writer"
	child := s.addTokenLocked(TokenSpec{Identity: subject, Role: role, Repos: in.Scope.Repos, Ops: in.Scope.Operations, Streams: in.Scope.Streams, TTL: ttl}, c.tok.id)
	cred := agwp.Credential{ID: child.id, Subject: subject, Kind: kindOf(subject), Description: in.Description, Scope: in.Scope,
		CreatedAtMs: s.nowMs(), ExpiresAtMs: child.expiresAt.UnixMilli(), CreatedBy: c.identity}
	return map[string]any{"credential": cred, "token": child.secret}, 0, nil
}

// sse serves GET .../events.
func (s *Server) sse(w http.ResponseWriter, req *http.Request, c *reqCtx) {
	fl, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "no flusher", 500)
		return
	}
	s.mu.Lock()
	r := c.repo
	after, hasAfter := uint64(0), false
	if v := c.q.Get("after_seq"); v != "" {
		after, _ = strconv.ParseUint(v, 10, 64)
		hasAfter = true
	}
	if v := req.Header.Get("Last-Event-ID"); v != "" && !hasAfter {
		after, _ = strconv.ParseUint(v, 10, 64)
		hasAfter = true
	}
	if !hasAfter {
		after = uint64(len(r.events))
	}
	topics := splitTopics(c.q.Get("topics"))
	hb := 15 * time.Second
	if ms, _ := strconv.Atoi(c.q.Get("heartbeat_ms")); ms >= 1000 {
		hb = time.Duration(ms) * time.Millisecond
	}
	s.sseLog = append(s.sseLog, SSEConnect{Repo: r.name, AfterSeq: c.q.Get("after_seq"), LastEventID: req.Header.Get("Last-Event-ID"), Topics: c.q.Get("topics")})
	s.streams++
	kill := s.kill
	s.mu.Unlock()
	defer func() {
		s.mu.Lock()
		s.streams--
		s.mu.Unlock()
	}()
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.WriteHeader(200)
	fl.Flush()
	timer := time.NewTimer(hb)
	defer timer.Stop()
	for {
		s.mu.Lock()
		var batch []agwp.Event
		for _, e := range r.events {
			if e.Seq > after && topicMatch(topics, e.Topic) {
				batch = append(batch, e)
			}
		}
		if n := len(r.events); n > 0 && uint64(n) > after {
			after = uint64(n)
		}
		notify := r.notify
		s.mu.Unlock()
		for _, e := range batch {
			b, _ := json.Marshal(e)
			fmt.Fprintf(w, "id: %d\nevent: %s\ndata: %s\n\n", e.Seq, e.Topic, b)
		}
		fl.Flush()
		if !timer.Stop() {
			select {
			case <-timer.C:
			default:
			}
		}
		timer.Reset(hb)
		select {
		case <-notify:
		case <-timer.C:
			fmt.Fprint(w, ": heartbeat\n\n")
			fl.Flush()
		case <-kill:
			return
		case <-req.Context().Done():
			return
		}
	}
}
