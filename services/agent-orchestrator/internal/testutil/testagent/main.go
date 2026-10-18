// Command testagent is a TEST-ONLY agent executable. It is not part of the
// orchestrator binary or its image: the tests build it (testutil.BuildTestAgent)
// and run it as an ordinary `type: command` executor, so the orchestrator
// drives it exactly as it drives any real agent, through the documented AG_*
// environment contract and the real AGWP API.
//
// It does what a well-behaved agent does and nothing clever:
//
//  1. creates a virtual workspace on the stream,
//  2. applies a fixed set of file changes as one changeset,
//  3. signs a provenance record with the agent's key and submits it,
//  4. opens a proposal for the task.
//
// It never integrates. It is configured through the executor's `env:`:
//
//	TESTAGENT_FILES      JSON object path -> content to write
//	TESTAGENT_DELETE     JSON array of paths to delete
//	TESTAGENT_TASK_FILES 1: a line "ag-script:{json}" in the task description overrides the above
//	TESTAGENT_MODEL / TESTAGENT_RATIONALE / TESTAGENT_TITLE / TESTAGENT_MESSAGE   free text
//	TESTAGENT_DELAY      a Go duration to wait before acting
package main

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

const scriptMarker = "ag-script:"

type script struct {
	Files   map[string]string `json:"files"`
	Delete  []string          `json:"delete"`
	Message string            `json:"message"`
	Title   string            `json:"title"`
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, "testagent:", err)
		os.Exit(1)
	}
}

func run() error {
	ctx := context.Background()
	env := os.Getenv
	var task agwp.Task
	if b, err := os.ReadFile(env("AG_TASK_FILE")); err != nil {
		return fmt.Errorf("task file: %w", err)
	} else if err := json.Unmarshal(b, &task); err != nil {
		return fmt.Errorf("task file: %w", err)
	}
	sc := script{Message: env("TESTAGENT_MESSAGE"), Title: env("TESTAGENT_TITLE")}
	if v := env("TESTAGENT_FILES"); v != "" {
		if err := json.Unmarshal([]byte(v), &sc.Files); err != nil {
			return fmt.Errorf("TESTAGENT_FILES: %w", err)
		}
	}
	if v := env("TESTAGENT_DELETE"); v != "" {
		if err := json.Unmarshal([]byte(v), &sc.Delete); err != nil {
			return fmt.Errorf("TESTAGENT_DELETE: %w", err)
		}
	}
	if env("TESTAGENT_TASK_FILES") == "1" {
		if ts, ok := parseScript(task.Description); ok {
			sc.Files, sc.Delete = ts.Files, ts.Delete
			if ts.Message != "" {
				sc.Message = ts.Message
			}
			if ts.Title != "" {
				sc.Title = ts.Title
			}
		}
	}
	if len(sc.Files) == 0 && len(sc.Delete) == 0 {
		return fmt.Errorf("nothing to do for task %s", task.ID)
	}
	if sc.Message == "" {
		sc.Message = "test agent change"
	}
	if sc.Title == "" {
		sc.Title = sc.Message
	}
	if d := env("TESTAGENT_DELAY"); d != "" {
		dur, err := time.ParseDuration(d)
		if err != nil {
			return fmt.Errorf("TESTAGENT_DELAY: %w", err)
		}
		time.Sleep(dur)
	}
	key, err := agentkey.Load(env("AG_KEY_FILE"))
	if err != nil {
		return fmt.Errorf("agent key: %w", err)
	}
	base, err := agwp.New(env("AG_SERVER"), secret.New(env("AG_TOKEN")), agwp.WithIdentityLabel("testagent"))
	if err != nil {
		return err
	}
	client := base.WithRun(env("AG_RUN"))

	paths := make([]string, 0, len(sc.Files))
	for p := range sc.Files {
		paths = append(paths, p)
	}
	sort.Strings(paths)
	var changes []agwp.FileChange
	for _, p := range paths {
		changes = append(changes, agwp.FileChange{Path: p, Op: "put", Content: []byte(sc.Files[p])})
	}
	for _, p := range sc.Delete {
		changes = append(changes, agwp.FileChange{Path: p, Op: "delete"})
	}
	repo, stream := env("AG_REPO"), env("AG_STREAM")
	ws := "ag-" + env("AG_RUN")
	if _, err := client.CreateWorkspace(ctx, repo, ws, stream); err != nil {
		return fmt.Errorf("create workspace: %w", err)
	}
	w, err := client.ModifyWorkspace(ctx, repo, ws, sc.Message, "", changes)
	if err != nil {
		return fmt.Errorf("modify workspace: %w", err)
	}
	digest := sha256.Sum256([]byte(task.Title + "\n" + task.Description))
	rec := agwp.ProvenanceRecord{
		Changeset: w.Head, Agent: env("AG_IDENTITY"), RunID: env("AG_RUN"), TaskID: task.ID,
		Model: env("TESTAGENT_MODEL"), PromptDigest: hex.EncodeToString(digest[:]), Rationale: env("TESTAGENT_RATIONALE"),
		ChecksRun: []string{}, Inputs: []string{"task:" + task.ID}, Parameters: map[string]string{"executor": "testagent"},
		CreatedAtMs: time.Now().UnixMilli(),
	}.Normalize()
	sig, err := key.Sign(agwp.ProvenanceDomain, rec.CanonicalBytes())
	if err != nil {
		return fmt.Errorf("sign provenance: %w", err)
	}
	if _, err := client.SubmitProvenance(ctx, repo, rec, sig); err != nil {
		return fmt.Errorf("submit provenance: %w", err)
	}
	p, err := client.CreateProposal(ctx, repo, agwp.ProposalInput{
		Stream: stream, Head: w.Head, Title: sc.Title, TaskID: task.ID,
		Description: "Opened by the test agent for task " + task.ID,
	})
	if err != nil {
		return fmt.Errorf("open proposal: %w", err)
	}
	if rf := env("AG_RESULT_FILE"); rf != "" {
		b, _ := json.Marshal(map[string]string{"summary": "test agent change " + w.Head[:min(12, len(w.Head))] + " proposed as " + p.ID})
		_ = os.WriteFile(rf, b, 0o600)
	}
	fmt.Printf("proposal %s opened for changeset %s\n", p.ID, w.Head)
	return nil
}

func parseScript(desc string) (script, bool) {
	for _, line := range strings.Split(desc, "\n") {
		if rest, ok := strings.CutPrefix(strings.TrimSpace(line), scriptMarker); ok {
			var s script
			dec := json.NewDecoder(strings.NewReader(strings.TrimSpace(rest)))
			dec.DisallowUnknownFields()
			if err := dec.Decode(&s); err != nil {
				return script{}, false
			}
			return s, true
		}
	}
	return script{}, false
}
