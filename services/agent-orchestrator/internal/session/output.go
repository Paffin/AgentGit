package session

import (
	"context"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

const savedOutputLimit = 32 * 1024

// Output capture stays off unless the operator explicitly enabled it for this executor.
func (s *Session) publishOutput() {
	if !s.req.Executor.PublishOutput {
		return
	}
	output := agwp.RunOutput{Sequence: 1, Stdout: s.redactedTail(s.stdout), Stderr: s.redactedTail(s.stderr),
		StdoutBytes: uint64(s.stdout.Total()), StderrBytes: uint64(s.stderr.Total())}
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	if err := s.m.o.Orch.SaveRunOutput(ctx, s.req.Repo, s.run.RunID, output); err != nil {
		s.m.o.Log.Warn("could not save opted-in executor output", "run", s.run.RunID, "err", err)
	}
}

func (s *Session) redactedTail(t *tail) string {
	text := s.redact(t.Last(0))
	text = strings.ToValidUTF8(text, "�")
	if len(text) > savedOutputLimit {
		text = text[len(text)-savedOutputLimit:]
		for len(text) > 0 && !utf8.RuneStart(text[0]) {
			text = text[1:]
		}
	}
	return text
}

func scrubEnvironment(text, name, value string) string {
	if value == "" {
		return text
	}
	sensitive := false
	for _, marker := range []string{"TOKEN", "SECRET", "PASSWORD", "KEY", "CREDENTIAL"} {
		sensitive = sensitive || strings.Contains(strings.ToUpper(name), marker)
	}
	if len(value) >= 8 || sensitive {
		return strings.ReplaceAll(text, value, secret.Redacted)
	}
	return text
}

func (s *Session) redact(text string) string {
	text = s.key.Scrub(secret.Scrub(text, s.token))

	for _, name := range s.req.Executor.Pass {
		text = scrubEnvironment(text, name, s.m.o.ParentEnv(name))
	}
	for name, value := range s.req.Executor.Env {
		text = scrubEnvironment(text, name, value)
	}
	return text
}
