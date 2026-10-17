// Command orchestrator is the AgentGit Agent Platform service: it schedules
// tasks onto agent sessions, keeps proposals prepared for the trusted execution
// plane, and posts advisory conflict suggestions and automated review comments.
// It is an AGWP client; it never touches storage and never integrates.
//
// Usage:
//
//	orchestrator [run] -config FILE     run the service (default command)
//	orchestrator check -config FILE     verify config, credentials and server without changing anything
//	orchestrator ops -config FILE       print the operations the service credential must carry (JSON)
//	orchestrator config -config FILE    print the effective configuration (no secrets)
//	orchestrator version
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/app"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
)

func main() { os.Exit(run(os.Args[1:], os.Getenv, os.Stdout, os.Stderr)) }

func run(args []string, getenv func(string) string, stdout, stderr io.Writer) int {
	cmd := "run"
	if len(args) > 0 && len(args[0]) > 0 && args[0][0] != '-' {
		cmd, args = args[0], args[1:]
	}
	if cmd == "version" || cmd == "-version" {
		fmt.Fprintln(stdout, "agent-orchestrator", app.Version)
		return 0
	}
	if cmd == "help" {
		fmt.Fprint(stdout, usage)
		return 0
	}
	fs := flag.NewFlagSet(cmd, flag.ContinueOnError)
	fs.SetOutput(stderr)
	path := fs.String("config", getenv("AG_ORCH_CONFIG"), "configuration file (YAML or JSON); default $AG_ORCH_CONFIG")
	asJSON := fs.Bool("json", false, "machine-readable output (check)")
	forWho := fs.String("for", "", "ops: print only one identity's operations (orchestrator, reviewer, advisor or agent)")
	asFlags := fs.Bool("flags", false, "ops: print the operations as --operation flags for 'ag-server admin mint' (needs -for)")
	if err := fs.Parse(args); err != nil {
		if err == flag.ErrHelp {
			return 0
		}
		return 2
	}
	if *path == "" {
		fmt.Fprintln(stderr, "orchestrator: no configuration file: pass -config or set AG_ORCH_CONFIG")
		return 2
	}
	load := config.Load
	if cmd == "ops" || cmd == "config" {
		load = config.LoadNoCredentials // these only describe the configuration; no token needed
	}
	cfg, err := load(*path, getenv)
	if err != nil {
		fmt.Fprintln(stderr, err)
		return 2
	}
	switch cmd {
	case "config":
		fmt.Fprint(stdout, cfg.Redacted())
		return 0
	case "ops":
		all := map[string][]string{
			"orchestrator": app.OrchestratorOps(cfg), "reviewer": app.ReviewerOps(), "advisor": app.AdvisorOps(),
			"agent": config.DefaultAgentOps,
		}
		if *asFlags && *forWho == "" {
			fmt.Fprintln(stderr, "orchestrator ops: -flags needs -for orchestrator|reviewer|advisor|agent")
			return 2
		}
		if *forWho != "" {
			ops, ok := all[*forWho]
			if !ok {
				fmt.Fprintf(stderr, "orchestrator ops: unknown identity %q (want orchestrator, reviewer, advisor or agent)\n", *forWho)
				return 2
			}
			if *asFlags {
				parts := make([]string, len(ops))
				for i, o := range ops {
					parts[i] = "--operation " + o
				}
				fmt.Fprintln(stdout, strings.Join(parts, " "))
				return 0
			}
			all = map[string][]string{*forWho: ops}
		}
		enc := json.NewEncoder(stdout)
		enc.SetIndent("", "  ")
		_ = enc.Encode(all)
		return 0
	case "check":
		return doCheck(cfg, getenv, *asJSON, stdout, stderr)
	case "run":
		return doRun(cfg, getenv, stderr)
	}
	fmt.Fprintf(stderr, "orchestrator: unknown command %q\n\n%s", cmd, usage)
	return 2
}

const usage = `usage: orchestrator [run|check|ops|config|version] [-config FILE]

  run      run the service (default)
  check    verify the configuration, credentials and server; changes nothing
  ops      print the operations each service credential must carry (JSON);
           -for orchestrator|reviewer|advisor|agent -flags prints them as
           --operation flags for 'ag-server admin mint'
  config   print the effective configuration (never contains secrets)
  version  print the version
`

func newLogger(c *config.Config, w io.Writer) *slog.Logger {
	var lvl slog.Level
	_ = lvl.UnmarshalText([]byte(c.Log.Level))
	opts := &slog.HandlerOptions{Level: lvl}
	if c.Log.Format == "json" {
		return slog.New(slog.NewJSONHandler(w, opts))
	}
	return slog.New(slog.NewTextHandler(w, opts))
}

func doCheck(cfg *config.Config, getenv func(string) string, asJSON bool, stdout, stderr io.Writer) int {
	a, err := app.New(app.Options{Config: cfg, Getenv: getenv, Log: newLogger(cfg, io.Discard)})
	if err != nil {
		fmt.Fprintln(stderr, "orchestrator:", err)
		return 2
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	rep := a.Check(ctx)
	if asJSON {
		enc := json.NewEncoder(stdout)
		enc.SetIndent("", "  ")
		_ = enc.Encode(map[string]any{"ok": rep.OK(), "items": rep.Items})
	} else {
		fmt.Fprint(stdout, rep.String())
	}
	if !rep.OK() {
		return 1
	}
	return 0
}

func doRun(cfg *config.Config, getenv func(string) string, stderr io.Writer) int {
	log := newLogger(cfg, stderr)
	a, err := app.New(app.Options{Config: cfg, Getenv: getenv, Log: log})
	if err != nil {
		log.Error("cannot start", "err", err)
		return 2
	}
	ctx, stop := signal.NotifyContext(context.Background(), syscall.SIGINT, syscall.SIGTERM)
	defer stop()
	go func() { // a second signal while shutting down forces the exit
		<-ctx.Done()
		stop()
		sig := make(chan os.Signal, 1)
		signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
		<-sig
		log.Error("second signal: exiting immediately")
		os.Exit(130)
	}()
	if err := a.Run(ctx); err != nil {
		log.Error("stopped with an error", "err", err)
		return 1
	}
	return 0
}
