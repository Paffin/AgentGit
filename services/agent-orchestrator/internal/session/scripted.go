package session

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"strings"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

// scriptedExecutor is a built-in stand-in for an agent, for tests and demos.
// It does what a well-behaved real agent does, through the same API and with
// the same restricted credential:
//
//  1. creates a virtual workspace on the stream,
//  2. applies a declared set of file changes as one changeset,
//  3. signs a provenance record with the agent's own key and submits it,
//  4. opens a proposal for the task.
//
// It never integrates; policy and humans decide what lands.
type scriptedExecutor struct{ cfg config.Scripted }

// scriptMarker starts the line of a task description that carries a JSON
// script (only honoured when task_files is on).
const scriptMarker = "ag-script:"

// taskScript is the JSON a task description may carry.
type taskScript struct {
	Files   map[string]string `json:"files"`
	Delete  []string          `json:"delete"`
	Message string            `json:"message"`
	Title   string            `json:"title"`
}

func (s scriptedExecutor) Run(ctx context.Context, env Env) Exit {
	fail := func(format string, a ...any) Exit {
		msg := fmt.Sprintf(format, a...)
		fmt.Fprintln(env.Stderr, msg)
		return Exit{Code: 1, Err: nil}
	}
	files, deletes := s.cfg.Files, []string(nil)
	message, title := s.cfg.Message, s.cfg.Title
	if s.cfg.TaskFiles {
		if ts, ok := parseTaskScript(env.Task.Description); ok {
			files, deletes = ts.Files, ts.Delete
			if ts.Message != "" {
				message = ts.Message
			}
			if ts.Title != "" {
				title = ts.Title
			}
		}
	}
	if len(files) == 0 && len(deletes) == 0 {
		return fail("scripted executor: nothing to do for task %s", env.Task.ID)
	}
	if s.cfg.Delay > 0 {
		if err := clock.Sleep(ctx, env.Clock, s.cfg.Delay); err != nil {
			return Exit{Code: 1, Err: err}
		}
	}
	key, err := agentkey.Load(env.KeyFile)
	if err != nil {
		return fail("scripted executor: cannot load the agent key: %v", err)
	}

	paths := make([]string, 0, len(files))
	for p := range files {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	changes := make([]agwp.FileChange, 0, len(paths)+len(deletes))
	for _, p := range paths {
		changes = append(changes, agwp.FileChange{Path: p, Op: "put", Content: []byte(files[p])})
	}
	for _, p := range deletes {
		changes = append(changes, agwp.FileChange{Path: p, Op: "delete"})
	}

	ws := "ag-" + env.RunID
	if _, err := env.Client.CreateWorkspace(ctx, env.Repo, ws, env.Stream); err != nil {
		return fail("scripted executor: create workspace: %v", err)
	}
	w, err := env.Client.ModifyWorkspace(ctx, env.Repo, ws, message, "", changes)
	if err != nil {
		return fail("scripted executor: modify workspace: %v", err)
	}

	digest := sha256.Sum256([]byte(env.Task.Title + "\n" + env.Task.Description))
	rec := agwp.ProvenanceRecord{
		Changeset: w.Head, Agent: env.Identity, RunID: env.RunID, TaskID: env.Task.ID,
		Model: s.cfg.Model, PromptDigest: hex.EncodeToString(digest[:]), Rationale: s.cfg.Rationale,
		ChecksRun: []string{}, Inputs: []string{"task:" + env.Task.ID}, Parameters: map[string]string{"executor": "scripted"},
		CreatedAtMs: env.Clock.Now().UnixMilli(),
	}.Normalize()
	sig, err := key.Sign(agwp.ProvenanceDomain, rec.CanonicalBytes())
	if err != nil {
		return fail("scripted executor: sign provenance: %v", err)
	}
	if _, err := env.Client.SubmitProvenance(ctx, env.Repo, rec, sig); err != nil {
		return fail("scripted executor: submit provenance: %v", err)
	}
	p, err := env.Client.CreateProposal(ctx, env.Repo, agwp.ProposalInput{
		Stream: env.Stream, Head: w.Head, Title: title, TaskID: env.Task.ID,
		Description: "Opened by the scripted executor for task " + env.Task.ID,
	})
	if err != nil {
		return fail("scripted executor: open proposal: %v", err)
	}
	if env.ResultFile != "" {
		b, _ := json.Marshal(map[string]string{"summary": "scripted change " + w.Head[:min(12, len(w.Head))] + " proposed as " + p.ID})
		_ = os.WriteFile(env.ResultFile, b, 0o600)
	}
	fmt.Fprintf(env.Stdout, "proposal %s opened for changeset %s\n", p.ID, w.Head)
	return Exit{}
}

// parseTaskScript finds the first line starting with scriptMarker and parses
// the JSON after it.
func parseTaskScript(desc string) (taskScript, bool) {
	for _, line := range strings.Split(desc, "\n") {
		line = strings.TrimSpace(line)
		if rest, ok := strings.CutPrefix(line, scriptMarker); ok {
			var ts taskScript
			dec := json.NewDecoder(strings.NewReader(strings.TrimSpace(rest)))
			dec.DisallowUnknownFields()
			if err := dec.Decode(&ts); err != nil {
				return taskScript{}, false
			}
			return ts, true
		}
	}
	return taskScript{}, false
}
