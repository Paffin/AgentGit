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
	s.writeOutput(context.Background(), false)
}

func (s *Session) startOutputUpdates(root context.Context) func() {
	if !s.req.Executor.PublishOutput || s.req.Executor.OutputInterval <= 0 {
		return func() {}
	}
	ctx, cancel := context.WithCancel(root)
	done := make(chan struct{})
	go func() {
		defer close(done)
		ticker := s.m.o.Clock.NewTicker(s.req.Executor.OutputInterval)
		defer ticker.Stop()
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C():
				if !s.writeOutput(ctx, true) {
					return
				}
			}
		}
	}()
	return func() { cancel(); <-done }
}

func (s *Session) writeOutput(parent context.Context, live bool) bool {
	if !s.req.Executor.PublishOutput || s.outputDisabled {
		return false
	}
	output := agwp.RunOutput{Stdout: s.outputTail(s.stdout, live), Stderr: s.outputTail(s.stderr, live),
		StdoutBytes: uint64(s.stdout.Total()), StderrBytes: uint64(s.stderr.Total())}
	if (live && output.StdoutBytes+output.StderrBytes == 0) || sameOutput(s.outputLast, &output) {
		return true
	}
	s.outputSequence++
	output.Sequence = s.outputSequence
	ctx, cancel := context.WithTimeout(parent, 10*time.Second)
	defer cancel()
	if err := s.m.o.Orch.SaveRunOutput(ctx, s.req.Repo, s.run.RunID, output); err != nil {
		if parent.Err() != nil {
			return false
		}
		if !s.outputFailureLogged {
			s.m.o.Log.Warn("could not save opted-in executor output", "run", s.run.RunID, "err", err)
			s.outputFailureLogged = true
		}
		s.outputDisabled = agwp.IsCode(err, agwp.CodeFailedPrecond, agwp.CodeNotFound, agwp.CodePermissionDenied, agwp.CodeUnauthenticated)
		return !s.outputDisabled
	}
	s.outputLast, s.outputFailureLogged = &output, false
	return true
}

func sameOutput(a, b *agwp.RunOutput) bool {
	return a != nil && a.Stdout == b.Stdout && a.Stderr == b.Stderr && a.StdoutBytes == b.StdoutBytes && a.StderrBytes == b.StderrBytes
}

func (s *Session) outputTail(t *tail, live bool) string {
	text := s.redact(t.Last(0))
	if live {
		text = text[:len(text)-s.pendingOutputSuffix(text)]
	}
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
	if !scrubbedEnvironment(name, value) {
		return text
	}
	return strings.ReplaceAll(text, value, secret.Redacted)
}

func scrubbedEnvironment(name, value string) bool {
	if value == "" {
		return false
	}
	sensitive := false
	for _, marker := range []string{"TOKEN", "SECRET", "PASSWORD", "KEY", "CREDENTIAL"} {
		sensitive = sensitive || strings.Contains(strings.ToUpper(name), marker)
	}
	return len(value) >= 8 || sensitive
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

func (s *Session) pendingOutputSuffix(text string) int {
	n := max(secret.PendingSuffixLength(text, s.token.Reveal()), s.key.PendingSuffixLength(text))
	for _, name := range s.req.Executor.Pass {
		value := s.m.o.ParentEnv(name)
		if scrubbedEnvironment(name, value) {
			n = max(n, secret.PendingSuffixLength(text, value))
		}
	}
	for name, value := range s.req.Executor.Env {
		if scrubbedEnvironment(name, value) {
			n = max(n, secret.PendingSuffixLength(text, value))
		}
	}
	return n
}
