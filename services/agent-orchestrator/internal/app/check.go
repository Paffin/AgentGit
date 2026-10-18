package app

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"sort"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

// CheckItem is one line of a check report.
type CheckItem struct {
	Level   string `json:"level"` // ok | warn | error
	Subject string `json:"subject"`
	Message string `json:"message"`
}

// CheckReport is the result of Check.
type CheckReport struct{ Items []CheckItem }

// OK is true when there is no error-level item.
func (r CheckReport) OK() bool {
	for _, i := range r.Items {
		if i.Level == "error" {
			return false
		}
	}
	return true
}

func (r *CheckReport) add(level, subject, format string, a ...any) {
	r.Items = append(r.Items, CheckItem{Level: level, Subject: subject, Message: fmt.Sprintf(format, a...)})
}

// String renders the report for people.
func (r CheckReport) String() string {
	var b strings.Builder
	for _, i := range r.Items {
		mark := map[string]string{"ok": "ok   ", "warn": "WARN ", "error": "ERROR"}[i.Level]
		fmt.Fprintf(&b, "%s %-24s %s\n", mark, i.Subject, i.Message)
	}
	return b.String()
}

// Check verifies that the configuration will work against the live server
// without changing anything: protocol, credentials (identity, roles, scope
// against what the configuration needs), repositories and streams, executors.
func (a *App) Check(ctx context.Context) CheckReport {
	var rep CheckReport
	caps, err := a.orch.Negotiate(ctx)
	if err != nil {
		rep.add("error", "server", "%v", err)
		return rep
	}
	rep.add("ok", "server", "%s speaks %s (server %s)", a.cfg.Server.URL, caps.ProtocolVersion, caps.ServerVersion)
	if !caps.Has("events.replay") {
		rep.add("error", "server", "events.replay is not advertised: the orchestrator needs the event feed")
	}
	if !caps.Has("auth.scoped_credentials") {
		rep.add("error", "server", "scoped credentials are not advertised: agents could not be given narrow credentials")
	}
	if !caps.Has("auth.run_credentials") {
		rep.add("error", "server", "run-bound credentials are not advertised: concurrent sessions of one agent could act for each other, and the orchestrator refuses to hand an executor a credential that is not bound to its run (upgrade ag-server)")
	}
	if !caps.Has("auth.delegation") || !caps.Has("agents.operate") {
		rep.add("error", "server", "the delegator role and the operator calls (finish a task, reap runs) are not advertised: the orchestrator would need an administrator credential (upgrade ag-server)")
	}

	type ident struct {
		label string
		c     *agwp.Client
		need  []string
		roles []string // any of these satisfies the role requirement
	}
	for _, id := range []ident{
		{"orchestrator", a.orch, OrchestratorOps(a.cfg), []string{"delegator", "admin"}},
		{"reviewer", a.reviewer, ReviewerOps(), []string{"reviewer", "admin"}},
		{"advisor", a.advisorClient, AdvisorOps(), []string{"reviewer", "admin"}},
	} {
		p, err := id.c.Whoami(ctx)
		if err != nil {
			rep.add("error", id.label, "credential rejected: %v", err)
			continue
		}
		a.mu.Lock()
		a.principals[id.label] = p
		a.mu.Unlock()
		rep.add("ok", id.label, "authenticates as %s (%s), roles %s", p.Identity, p.Method, strings.Join(p.Roles, ", "))
		if p.Kind != "service" {
			rep.add("warn", id.label, "authenticates as a %s, not a service identity: audit trails and policies are clearer with service:%s", p.Kind, id.label)
		}
		for _, repo := range a.repos {
			if !hasRole(p.Roles, repo.cfg.Name, id.roles) {
				rep.add("error", id.label, "no %s role on %s (roles: %s)", strings.Join(id.roles, " or "), repo.cfg.Name, strings.Join(p.Roles, ", "))
			}
		}
		switch {
		case len(p.Scope.Operations) == 0:
			rep.add("warn", id.label, "the credential is unrestricted: it carries every power of its roles. Mint a scoped one (`orchestrator ops` prints the operations to request)")
		default:
			have := map[string]bool{}
			for _, o := range p.Scope.Operations {
				have[o] = true
			}
			var missing []string
			for _, o := range id.need {
				if !have[o] {
					missing = append(missing, o)
				}
			}
			if len(missing) > 0 {
				hint := ""
				if env := a.shared[id.label]; env != "" {
					hint = fmt.Sprintf(" (%s is not set, so the orchestrator's credential is used; mint a separate one with `orchestrator ops`, or disable what needs it)", env)
				}
				rep.add("error", id.label, "the credential's scope lacks: %s%s", strings.Join(missing, ", "), hint)
			} else {
				rep.add("ok", id.label, "the credential's scope covers the %d operations this configuration needs", len(id.need))
			}
			var extra []string
			need := map[string]bool{}
			for _, o := range id.need {
				need[o] = true
			}
			for _, o := range p.Scope.Operations {
				if !need[o] {
					extra = append(extra, o)
				}
			}
			sort.Strings(extra)
			if len(extra) > 0 {
				rep.add("warn", id.label, "the credential also carries operations this configuration does not use: %s", strings.Join(extra, ", "))
			}
		}
		if !p.ExpiresAt().IsZero() {
			if left := time.Until(p.ExpiresAt()); left < 72*time.Hour {
				rep.add("warn", id.label, "the credential expires in %s", left.Round(time.Minute))
			} else {
				rep.add("ok", id.label, "the credential expires %s", p.ExpiresAt().Format(time.RFC3339))
			}
		}
	}

	for _, rt := range a.repos {
		if _, err := a.orch.DescribeRepo(ctx, rt.cfg.Name); err != nil {
			rep.add("error", rt.cfg.Name, "repository: %v", err)
			continue
		}
		if st, err := a.orch.ReadHead(ctx, rt.cfg.Name, rt.cfg.Stream); err != nil {
			rep.add("error", rt.cfg.Name, "stream %q: %v", rt.cfg.Stream, err)
		} else {
			rep.add("ok", rt.cfg.Name, "stream %q at version %d", rt.cfg.Stream, st.Version)
		}
	}
	for _, e := range a.cfg.Execs {
		switch e.Type {
		case "command":
			if _, err := exec.LookPath(e.Command[0]); err != nil {
				rep.add("warn", "executor "+e.Name, "%q was not found: %v", e.Command[0], err)
			} else {
				rep.add("ok", "executor "+e.Name, "command %q found, agent %q", e.Command[0], e.Agent)
			}
		default:
			rep.add("ok", "executor "+e.Name, "built-in scripted executor, agent %q", e.Agent)
		}
	}
	if err := os.MkdirAll(a.cfg.State, 0o700); err != nil {
		rep.add("error", "state_dir", "%v", err)
	} else {
		rep.add("ok", "state_dir", "%s is usable", a.cfg.State)
	}
	return rep
}

// hasRole reports whether roles ("pattern:role") grant one of want on repo.
func hasRole(roles []string, repo string, want []string) bool {
	for _, r := range roles {
		i := strings.LastIndex(r, ":")
		if i < 0 {
			continue
		}
		pattern, role := r[:i], r[i+1:]
		ok := false
		for _, w := range want {
			if role == w {
				ok = true
			}
		}
		if ok && matchPattern(pattern, repo) {
			return true
		}
	}
	return false
}

func matchPattern(pattern, s string) bool {
	if pattern == "*" || pattern == s {
		return true
	}
	if strings.HasSuffix(pattern, "*") {
		return strings.HasPrefix(s, strings.TrimSuffix(pattern, "*"))
	}
	return false
}
