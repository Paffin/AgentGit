// Package session runs agent sessions: one session is one agent run, from
// registering the agent and starting the run, through a scoped short-lived
// credential and an executor process, to completing the run.
//
// Trust boundary. The executor receives credentials minted for the agent
// (narrow scope, at most an hour, revoked when the session ends) and never the
// orchestrator's own. Everything the executor produces reaches the repository
// as proposals, reviews and validations that the server's policy judges; the
// orchestrator never integrates anything.
package session

import (
	"errors"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

// Errors a caller can react to.
var (
	// ErrNoSlot: the executor already runs its maximum number of sessions.
	ErrNoSlot = errors.New("session: no free slot for this executor")
	// ErrTaskUnavailable: the server refused to start a run on the task
	// (owned by someone else, blocked by dependencies, finished).
	ErrTaskUnavailable = errors.New("session: the task cannot be started")
	// ErrLeaseConflict: an exclusive lease on the task scope is held by someone else.
	ErrLeaseConflict = errors.New("session: lease conflict on the task scope")
	// ErrClosed: the manager is shutting down.
	ErrClosed = errors.New("session: manager is shutting down")
)

// Outcome says how a session ended.
type Outcome string

const (
	// OutcomeCompleted: the executor exited 0.
	OutcomeCompleted Outcome = "completed"
	// OutcomeFailed: non-zero exit, killed by a signal, or could not be launched.
	OutcomeFailed Outcome = "failed"
	// OutcomeTimeout: the executor exceeded its time limit and was stopped.
	OutcomeTimeout Outcome = "timeout"
	// OutcomeCancelled: the orchestrator stopped the executor (shutdown, task cancelled).
	OutcomeCancelled Outcome = "cancelled"
)

// Disposition is what should happen to the task when a session ends.
type Disposition int

const (
	// Keep leaves the task as it is (in progress): retry later, or wait for a proposal to land.
	Keep Disposition = iota
	// Done marks the task done.
	Done
	// Cancel marks the task cancelled.
	Cancel
)

func (d Disposition) String() string { return [...]string{"keep", "done", "cancel"}[d] }

// Result is the report of a finished session.
type Result struct {
	Repo, Executor, Agent, RunID string
	Task                         agwp.Task
	Attempt                      int
	Outcome                      Outcome
	ExitCode                     int
	Signal                       string
	Duration                     time.Duration
	Summary                      string
	// Proposals opened by this run (any state), by id.
	Proposals []agwp.Proposal
	// Reason is why the orchestrator stopped the executor, when it did.
	Reason string
}

// Request asks for a session.
type Request struct {
	Repo     string
	Stream   string
	Task     agwp.Task
	Executor config.Executor
	Attempt  int
	Lease    string // soft | exclusive | none
	LeaseTTL time.Duration
	// Decide chooses the task's fate once the session ended. Called before the
	// session's credential is revoked, so the verdict can be applied as the
	// task's owner. nil means Keep.
	Decide func(Result) Disposition
}

// LeaseInfo is a lease held by a session.
type LeaseInfo struct {
	ID        string `json:"id"`
	Scope     string `json:"scope"`
	Exclusive bool   `json:"exclusive"`
	Token     uint64 `json:"fencing_token"`
}

// Info is a snapshot of a running session (it contains no secrets).
type Info struct {
	Repo          string      `json:"repo"`
	Executor      string      `json:"executor"`
	Agent         string      `json:"agent"`
	RunID         string      `json:"run_id"`
	TaskID        string      `json:"task_id"`
	TaskTitle     string      `json:"task_title"`
	Scope         []string    `json:"scope"`
	Attempt       int         `json:"attempt"`
	State         string      `json:"state"` // running | stopping
	StartedAt     time.Time   `json:"started_at"`
	LastHeartbeat time.Time   `json:"last_heartbeat,omitempty"`
	Leases        []LeaseInfo `json:"leases,omitempty"`
	StopReason    string      `json:"stop_reason,omitempty"`
	StderrTail    string      `json:"stderr_tail,omitempty"`
}
