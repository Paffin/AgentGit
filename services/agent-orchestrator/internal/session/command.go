package session

import (
	"context"
	"errors"
	"fmt"
	"os/exec"
	"sort"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

// commandExecutor runs an external program. The program is executed directly
// (no shell), in its own process group, with an explicit environment: only the
// variables the configuration passes through plus the AG_* contract below.
//
//	AG_SERVER       base URL of the AGWP server
//	AG_TOKEN        the agent's scoped credential (valid for at most an hour)
//	AG_RUN          the run id; send it as the "ag-run" header
//	AG_REPO         owner/name of the repository
//	AG_STREAM       the stream the work is meant for
//	AG_TASK         the task id
//	AG_TASK_FILE    path of a JSON file with the task (title and description are untrusted text)
//	AG_AGENT        the agent name
//	AG_IDENTITY     agent:<name>#<run>, for the ag CLI
//	AG_KEY_FILE     path of the agent's Ed25519 key, for signing provenance
//	AG_WORKDIR      private scratch directory (also HOME)
//	AG_RESULT_FILE  optional: write {"summary": "..."} here
type commandExecutor struct{ cfg config.Executor }

func (c commandExecutor) Run(ctx context.Context, env Env) Exit {
	argv := c.cfg.Command
	cmd := exec.Command(argv[0], argv[1:]...)
	cmd.Dir = env.WorkDir
	if c.cfg.Dir != "" {
		cmd.Dir = strings.ReplaceAll(c.cfg.Dir, "{workdir}", env.WorkDir)
	}
	cmd.Env = c.environ(env)
	cmd.Stdout, cmd.Stderr = env.Stdout, env.Stderr
	cmd.WaitDelay = 2 * time.Second // a grandchild holding the output pipes must not hang us
	setProcessGroup(cmd)
	if err := cmd.Start(); err != nil {
		return Exit{Code: -1, Err: fmt.Errorf("cannot start %q: %w", argv[0], err)}
	}
	pid := cmd.Process.Pid
	waitCh := make(chan error, 1)
	go func() { waitCh <- cmd.Wait() }()

	var waitErr error
	select {
	case waitErr = <-waitCh:
	case <-ctx.Done():
		terminateGroup(pid)
		t := env.Clock.NewTimer(env.Grace)
		select {
		case waitErr = <-waitCh:
			t.Stop()
		case <-t.C():
			killGroup(pid)
			waitErr = <-waitCh
		}
	}
	killGroup(pid) // nothing may outlive the session
	return exitOf(cmd, waitErr)
}

func exitOf(cmd *exec.Cmd, waitErr error) Exit {
	if cmd.ProcessState == nil {
		return Exit{Code: -1, Err: waitErr}
	}
	code, sig := exitDetails(cmd.ProcessState)
	ex := Exit{Code: code, Signal: sig}
	var ee *exec.ExitError
	if waitErr != nil && !errors.As(waitErr, &ee) && !errors.Is(waitErr, exec.ErrWaitDelay) {
		ex.Err = waitErr
	}
	return ex
}

// environ builds the child's environment from scratch.
func (c commandExecutor) environ(env Env) []string {
	vars := map[string]string{"HOME": env.WorkDir}
	for _, name := range c.cfg.Pass {
		if v := env.ParentEnv(name); v != "" {
			vars[name] = v
		}
	}
	for k, v := range c.cfg.Env {
		vars[k] = v
	}
	for k, v := range map[string]string{
		"AG_SERVER": env.Server, "AG_TOKEN": env.Token.Reveal(), "AG_RUN": env.RunID, "AG_REPO": env.Repo,
		"AG_STREAM": env.Stream, "AG_TASK": env.Task.ID, "AG_TASK_FILE": env.TaskFile, "AG_AGENT": env.Agent,
		"AG_IDENTITY": env.Identity, "AG_KEY_FILE": env.KeyFile, "AG_WORKDIR": env.WorkDir, "AG_RESULT_FILE": env.ResultFile,
	} {
		vars[k] = v
	}
	keys := make([]string, 0, len(vars))
	for k := range vars {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	out := make([]string, 0, len(keys))
	for _, k := range keys {
		out = append(out, k+"="+vars[k])
	}
	return out
}
