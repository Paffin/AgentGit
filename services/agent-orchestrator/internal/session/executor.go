package session

import (
	"context"
	"io"
	"log/slog"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// Env is everything an executor may know about its session. Note what is
// missing: the orchestrator's own credentials. Token is the agent's scoped,
// short-lived credential.
type Env struct {
	Clock clock.Clock
	Log   *slog.Logger

	Repo, Stream string
	Task         agwp.Task
	Agent        string // agent name
	RunID        string
	Identity     string // agent:<name>#<run>

	Server string
	Token  secret.Secret

	KeyFile    string // the agent's Ed25519 key (JSON, mode 0600)
	WorkDir    string // private scratch directory of this session
	TaskFile   string // JSON description of the task (untrusted text inside)
	ResultFile string // optional JSON result the executor may write

	// Client speaks AGWP with the agent credential and acts as the run.
	Client *agwp.Client

	Stdout, Stderr io.Writer
	Grace          time.Duration
	ParentEnv      func(string) string
}

// Exit is how an executor ended.
type Exit struct {
	// Code is the process exit status; -1 when it did not exit normally.
	Code int
	// Signal names the signal that killed the process, if any.
	Signal string
	// Err is a launch or internal failure (the process may never have run).
	Err error
}

// OK reports a clean exit.
func (e Exit) OK() bool { return e.Err == nil && e.Code == 0 && e.Signal == "" }

// Executor runs the agent. Cancelling ctx asks it to stop gracefully (the
// command executor sends SIGTERM to the process group, then SIGKILL after
// Env.Grace); Run returns only after the work has stopped.
type Executor interface {
	Run(ctx context.Context, env Env) Exit
}

// Exit codes with a meaning to the orchestrator. Everything else non-zero is
// a plain failure.
const (
	// ExitOK: success.
	ExitOK = 0
	// ExitTempFail (EX_TEMPFAIL): a temporary failure; counts as a failed
	// attempt but is never turned into a task cancellation before the attempt
	// budget is spent.
	ExitTempFail = 75
)
