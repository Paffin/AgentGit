// Package app wires the orchestrator: it builds the AGWP clients, the session
// manager and the per-repository components from a validated configuration,
// runs them, and serves the admin and observability endpoints.
package app

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"sync/atomic"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/advisor"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/clock"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/events"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/execsched"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/llm"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/metrics"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/review"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/scheduler"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/semindex"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/session"
)

// Version is set at build time (-ldflags "-X .../internal/app.Version=...").
var Version = "dev"

// Options are the dependencies a test may replace.
type Options struct {
	Config *config.Config
	Getenv func(string) string
	Log    *slog.Logger
	Clock  clock.Clock
	// Listener, when set, is served instead of listening on Config.Listen
	// (tests use port 0).
	Listener net.Listener
}

// repoRuntime holds the components of one repository.
type repoRuntime struct {
	cfg       config.Repo
	scheduler *scheduler.Scheduler
	exec      *execsched.Scheduler
	execShown bool
	advisor   *advisor.Component
	review    *review.Component
	index     *semindex.Component
}

// App is a running orchestrator.
type App struct {
	cfg     *config.Config
	getenv  func(string) string
	log     *slog.Logger
	clk     clock.Clock
	metrics *metrics.Service
	started time.Time

	orch, reviewer, advisorClient *agwp.Client
	llm                           *llm.Client       // nil unless a component runs in model mode
	shared                        map[string]string // label -> unset env var, for identities that fall back to the orchestrator's credential

	hub      *events.Hub
	mgr      *session.Manager
	repos    []*repoRuntime
	byName   map[string]*repoRuntime
	registry *semindex.Registry
	listener net.Listener
	srv      *http.Server
	adminTok secret.Secret

	preflightOK atomic.Bool
	mu          sync.Mutex
	principals  map[string]*agwp.Principal
	preflightEr string
}

// New builds an App; nothing runs until Run.
func New(o Options) (*App, error) {
	cfg := o.Config
	if cfg == nil {
		return nil, errors.New("app: no configuration")
	}
	getenv := o.Getenv
	if getenv == nil {
		getenv = os.Getenv
	}
	log := o.Log
	if log == nil {
		log = slog.Default()
	}
	clk := o.Clock
	if clk == nil {
		clk = clock.Real{}
	}
	a := &App{cfg: cfg, getenv: getenv, log: log, clk: clk, started: clk.Now(), metrics: metrics.NewService(metrics.NewRegistry()),
		byName: map[string]*repoRuntime{}, registry: semindex.NewRegistry(), listener: o.Listener, principals: map[string]*agwp.Principal{},
		shared: map[string]string{}}
	if cfg.Admin.TokenEnv != "" {
		a.adminTok = secret.New(getenv(cfg.Admin.TokenEnv))
	}
	if err := a.buildClients(); err != nil {
		return nil, err
	}
	if err := a.buildLLM(); err != nil {
		return nil, err
	}
	if err := a.build(); err != nil {
		return nil, err
	}
	return a, nil
}

// Metrics exposes the registry (tests read counters from it).
func (a *App) Metrics() *metrics.Service { return a.metrics }

func (a *App) httpClients() (*http.Client, *http.Client, error) {
	mk := func() (*http.Client, error) {
		t := http.DefaultTransport.(*http.Transport).Clone()
		t.MaxIdleConnsPerHost = 8
		if a.cfg.Server.CAFile != "" {
			pem, err := os.ReadFile(a.cfg.Server.CAFile)
			if err != nil {
				return nil, fmt.Errorf("server.ca_file: %w", err)
			}
			pool := x509.NewCertPool()
			if !pool.AppendCertsFromPEM(pem) {
				return nil, fmt.Errorf("server.ca_file %s holds no PEM certificate", a.cfg.Server.CAFile)
			}
			t.TLSClientConfig = &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
		}
		return &http.Client{Transport: t, CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }}, nil
	}
	req, err := mk()
	if err != nil {
		return nil, nil, err
	}
	stream, err := mk()
	return req, stream, err
}

func (a *App) buildClients() error {
	hc, sc, err := a.httpClients()
	if err != nil {
		return err
	}
	mk := func(label, env string) (*agwp.Client, error) {
		return agwp.New(a.cfg.Server.URL, secret.New(a.getenv(env)),
			agwp.WithHTTPClient(hc), agwp.WithStreamClient(sc), agwp.WithClock(a.clk), agwp.WithObserver(a.metrics),
			agwp.WithIdentityLabel(label), agwp.WithTimeout(a.cfg.Server.Timeout), agwp.WithMaxResponseBytes(a.cfg.Server.MaxResponse),
			agwp.WithRetry(agwp.RetryPolicy{MaxAttempts: a.cfg.Server.RetryMax, BaseDelay: 250 * time.Millisecond, MaxDelay: 5 * time.Second, MaxRetryAfter: 30 * time.Second}),
			agwp.WithUserAgent("agentgit-orchestrator/"+Version))
	}
	if a.orch, err = mk("orchestrator", a.cfg.Server.TokenEnv); err != nil {
		return err
	}
	revEnv, advEnv := a.cfg.Server.ReviewerEnv, a.cfg.Server.AdvisorEnv
	if a.getenv(revEnv) == "" {
		a.shared["reviewer"] = revEnv
		if a.identityEnabled("reviewer") {
			a.log.Warn("no reviewer credential: automated reviews are posted with the orchestrator's identity (set "+revEnv+" for a separate service:reviewer)", "env", revEnv)
		}
		revEnv = a.cfg.Server.TokenEnv
	}
	if a.reviewer, err = mk("reviewer", revEnv); err != nil {
		return err
	}
	if a.getenv(advEnv) == "" {
		a.shared["advisor"] = advEnv
		advEnv = revEnv
	}
	a.advisorClient, err = mk("advisor", advEnv)
	return err
}

// buildLLM creates the one language-model client the model-mode components share (so that its budget and concurrency
// bounds are global). The key is read from the environment variable the configuration NAMES; it is never logged.
func (a *App) buildLLM() error {
	if !a.cfg.UsesModel() {
		return nil
	}
	l := a.cfg.LLM
	hc := &http.Client{Timeout: 0, Transport: http.DefaultTransport.(*http.Transport).Clone()}
	if l.CAFile != "" {
		pem, err := os.ReadFile(l.CAFile)
		if err != nil {
			return fmt.Errorf("llm.ca_file: %w", err)
		}
		pool := x509.NewCertPool()
		if !pool.AppendCertsFromPEM(pem) {
			return fmt.Errorf("llm.ca_file %s holds no PEM certificate", l.CAFile)
		}
		hc.Transport.(*http.Transport).TLSClientConfig = &tls.Config{RootCAs: pool, MinVersion: tls.VersionTLS12}
	}
	var key secret.Secret
	if l.APIKeyEnv != "" {
		key = secret.New(a.getenv(l.APIKeyEnv))
	}
	c, err := llm.New(llm.Config{BaseURL: l.BaseURL, Model: l.Model, APIKey: key, Headers: l.Headers, Timeout: l.Timeout, Retries: l.RetryCount(),
		MaxOutputTokens: l.MaxOutputTokens, Temperature: l.Temp(), MaxInputBytes: l.MaxInputBytes, JSONMode: l.JSONMode, Concurrency: l.Concurrency,
		Budget:     llm.Budget{RequestsPerHour: l.RequestsPerHourLimit(), TokensPerDay: l.TokensPerDayLimit()},
		HTTPClient: hc, Clock: a.clk, Observer: a.metrics, Log: a.log.With("component", "llm")})
	if err != nil {
		return err
	}
	a.llm = c
	a.log.Info("language-model components enabled", "model", l.Model, "base_url", l.BaseURL, "key_from_env", l.APIKeyEnv != "")
	return nil
}

func (a *App) build() error {
	cfg := a.cfg
	keys, err := agentkey.OpenStore(filepath.Join(cfg.State, "keys"))
	if err != nil {
		return fmt.Errorf("key store: %w", err)
	}
	a.hub = events.NewHub(a.log, a.metrics, a.clk)
	a.mgr, err = session.NewManager(session.Options{
		Orch: a.orch, ServerURL: cfg.Server.URL, Keys: keys, WorkRoot: filepath.Join(cfg.State, "work"), Clock: a.clk, Metrics: a.metrics, Log: a.log,
		ParentEnv: a.getenv, OnDone: func(repo string) {
			if rt := a.byName[repo]; rt != nil && rt.scheduler != nil {
				rt.scheduler.Kick()
			}
		},
	})
	if err != nil {
		return err
	}
	logger := func(repo, comp string) *slog.Logger { return a.log.With("repo", repo, "component", comp) }
	for _, rc := range cfg.Repos {
		rt := &repoRuntime{cfg: rc}
		needExec := config.On(rc.ExecSched.Enabled) || config.On(rc.Advisor.Enabled) || config.On(rc.Schedule.Enabled)
		if needExec {
			erc := rc
			if !config.On(rc.ExecSched.Enabled) {
				off := false
				erc.ExecSched.Enqueue = &off // only here for the advisor and for placement: never queue jobs the operator switched off
			}
			rt.exec = execsched.New(execsched.Options{Repo: erc, API: a.orch, Clock: a.clk, Metrics: a.metrics, Log: logger(rc.Name, "execsched"), Loop: cfg.Loop,
				Refresh: rc.ExecSched.Refresh, Recent: rc.ExecSched.Recent, JobsLimit: rc.ExecSched.JobsLimit})
			rt.execShown = config.On(rc.ExecSched.Enabled)
		}
		if config.On(rc.Schedule.Enabled) && len(cfg.Execs) > 0 {
			var src scheduler.PathSource
			if rt.exec != nil {
				src = rt.exec
			}
			rt.scheduler, err = scheduler.New(scheduler.Options{Repo: rc, Execs: cfg.Execs, API: a.orch, Sessions: a.mgr, Paths: src,
				Clock: a.clk, Metrics: a.metrics, Log: a.log, Loop: cfg.Loop})
			if err != nil {
				return fmt.Errorf("%s: %w", rc.Name, err)
			}
		}
		if config.On(rc.Advisor.Enabled) {
			var adv advisor.Advisor // nil: the rules
			if rc.Advisor.Mode == config.ModeModel {
				adv = advisor.ModelAdvisor{LLM: a.llm, Rules: advisor.Rules{MaxFileBytes: rc.Advisor.MaxFile}, PromptFileBytes: rc.Advisor.ModelFileBytes}
			}
			rt.advisor = advisor.New(advisor.Options{Repo: rc, API: a.advisorClient, Advisor: adv, Clock: a.clk, Metrics: a.metrics, Log: a.log})
			rt.exec.Subscribe(rt.advisor.OnPrepared)
			rt.exec.SubscribeForget(rt.advisor.Forget)
		}
		if config.On(rc.Review.Enabled) {
			ro := review.Options{Repo: rc, API: a.reviewer, Reviewer: defaultReviewerIdentity, Clock: a.clk, Metrics: a.metrics, Log: a.log, Loop: cfg.Loop}
			if rc.Review.Mode == config.ModeModel {
				ro.Model = review.LLMReviewer{LLM: a.llm, DiffBytes: rc.Review.ModelDiffBytes, MaxFindings: rc.Review.ModelMaxFindings}
			}
			rt.review, err = review.New(ro)
			if err != nil {
				return fmt.Errorf("%s: %w", rc.Name, err)
			}
		}
		if config.On(rc.Index.Enabled) {
			store, serr := semindex.OpenStore(filepath.Join(cfg.State, "index"), rc.Name)
			if serr != nil {
				return fmt.Errorf("%s: %w", rc.Name, serr)
			}
			io := semindex.Options{Repo: rc, API: a.orch, Store: store, Clock: a.clk, Metrics: a.metrics, Log: a.log, Loop: cfg.Loop}
			if rc.Index.Mode == config.ModeModel {
				io.Judge = semindex.LLMJudge{LLM: a.llm, PatchBytes: rc.Index.JudgePatchBytes}
				io.JudgePerPass = rc.Index.JudgePerPass
			}
			rt.index, err = semindex.New(io)
			if err != nil {
				return fmt.Errorf("%s: %w", rc.Name, err)
			}
			a.registry.Add(rt.index)
		}
		a.repos = append(a.repos, rt)
		a.byName[rc.Name] = rt
	}
	a.subscribe()
	return nil
}

// subscribe routes repository events to the components that care.
func (a *App) subscribe() {
	route := func(name string, topics []string, fn func(rt *repoRuntime, ev agwp.Event)) {
		a.hub.Subscribe(name, topics, func(repo string, ev agwp.Event) {
			if rt := a.byName[repo]; rt != nil {
				fn(rt, ev)
			}
		})
	}
	route("scheduler", []string{"task", "run", "lease", "proposal", "stream.integrate"}, func(rt *repoRuntime, _ agwp.Event) {
		if rt.scheduler != nil {
			rt.scheduler.Kick()
		}
	})
	route("execsched", execsched.Topics(), func(rt *repoRuntime, ev agwp.Event) {
		if rt.exec != nil {
			rt.exec.OnEvent(ev)
		}
	})
	route("review", review.Topics(), func(rt *repoRuntime, ev agwp.Event) {
		if rt.review != nil {
			rt.review.OnEvent(ev)
		}
	})
	route("semindex", semindex.Topics(), func(rt *repoRuntime, ev agwp.Event) {
		if rt.index != nil {
			rt.index.OnEvent(ev)
		}
	})
}

// defaultReviewerIdentity is what a review comment says about its author until the preflight check has asked the
// server who the reviewer credential really is.
const defaultReviewerIdentity = "service:reviewer"

// Preflight checks the server speaks our protocol, that the credentials work
// and that every configured repository is reachable. It records the
// orchestrator's identity and the expiry of its credential.
func (a *App) Preflight(ctx context.Context) error {
	caps, err := a.orch.Negotiate(ctx)
	if err != nil {
		return fmt.Errorf("negotiate: %w", err)
	}
	if !caps.Has("events.replay") {
		return errors.New("the server does not advertise events.replay: the orchestrator needs the event feed")
	}
	for label, c := range map[string]*agwp.Client{"orchestrator": a.orch, "reviewer": a.reviewer, "advisor": a.advisorClient} {
		p, err := c.Whoami(ctx)
		if err != nil {
			return fmt.Errorf("%s credential: %w", label, err)
		}
		a.mu.Lock()
		a.principals[label] = p
		a.mu.Unlock()
		if label == "reviewer" {
			for _, rt := range a.repos {
				if rt.review != nil {
					rt.review.SetReviewer(p.Identity) // the comments name the identity the server knows, not a guess
				}
			}
		}
		exp := 0.0
		if !p.ExpiresAt().IsZero() {
			exp = float64(p.ExpiresAt().Unix())
			if left := time.Until(p.ExpiresAt()); left < 72*time.Hour {
				a.log.Warn("a service credential expires soon", "identity", p.Identity, "label", label, "expires_at", p.ExpiresAt(), "left", left.Round(time.Minute))
			}
		}
		a.metrics.CredentialExpiry.With(label).Set(exp)
	}
	for _, rt := range a.repos {
		if _, err := a.orch.DescribeRepo(ctx, rt.cfg.Name); err != nil {
			return fmt.Errorf("repository %s: %w", rt.cfg.Name, err)
		}
	}
	return nil
}

// Run starts every component and blocks until ctx is cancelled, then shuts
// down: new sessions stop first, running ones finish or are cancelled
// according to shutdown.mode, and the HTTP server goes last so /status keeps
// answering while that happens.
func (a *App) Run(ctx context.Context) error {
	if err := a.startHTTP(); err != nil {
		return err
	}
	var wg sync.WaitGroup
	start := func(ctx context.Context, f func(context.Context)) {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if a.waitReady(ctx) { // components start once the server has accepted our credentials
				f(ctx)
			}
		}()
	}
	rootCtx, stopRoot := context.WithCancel(context.Background())
	schedCtx, stopSched := context.WithCancel(context.Background())
	defer stopRoot()
	defer stopSched()

	// Wait for the server (retrying), then start the loops.
	go a.preflightLoop(rootCtx)
	for _, rt := range a.repos {
		rt := rt
		start(rootCtx, func(c context.Context) { a.hub.Follow(c, a.orch, rt.cfg.Name, a.cfg.Server.StreamIdle) })
		if rt.exec != nil {
			start(rootCtx, rt.exec.Run)
		}
		if rt.scheduler != nil {
			start(schedCtx, rt.scheduler.Run)
		}
		if rt.advisor != nil {
			start(rootCtx, rt.advisor.Run)
		}
		if rt.review != nil {
			start(rootCtx, rt.review.Run)
		}
		if rt.index != nil {
			start(rootCtx, rt.index.Run)
		}
	}
	start(rootCtx, a.watchCredentials)
	a.log.Info("orchestrator running", "version", Version, "listen", a.listenAddr(), "repos", len(a.repos), "executors", len(a.cfg.Execs))

	<-ctx.Done()
	a.log.Info("shutting down", "mode", a.cfg.Shutdown.Mode, "grace", a.cfg.Shutdown.Grace)
	stopSched() // no new sessions
	gctx, cancel := context.WithTimeout(context.Background(), a.cfg.Shutdown.Grace)
	a.mgr.Shutdown(gctx, a.cfg.Shutdown.Mode == "wait")
	cancel()
	stopRoot()
	wg.Wait()
	sctx, scancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer scancel()
	err := a.srv.Shutdown(sctx)
	a.log.Info("stopped")
	return err
}

// waitReady blocks until the preflight succeeded; false if ctx ended first.
func (a *App) waitReady(ctx context.Context) bool {
	for !a.preflightOK.Load() {
		if clock.Sleep(ctx, a.clk, 100*time.Millisecond) != nil {
			return false
		}
	}
	return true
}

func (a *App) preflightLoop(ctx context.Context) {
	delay := time.Second
	for {
		pctx, cancel := context.WithTimeout(ctx, 30*time.Second)
		err := a.Preflight(pctx)
		cancel()
		if ctx.Err() != nil {
			return
		}
		a.mu.Lock()
		a.preflightEr = ""
		if err != nil {
			a.preflightEr = err.Error()
		}
		a.mu.Unlock()
		if err == nil {
			if !a.preflightOK.Swap(true) {
				a.log.Info("connected to the server")
			}
			return
		}
		a.preflightOK.Store(false)
		a.log.Error("cannot reach or authenticate to the server; retrying", "err", err, "in", delay)
		if clock.Sleep(ctx, a.clk, delay) != nil {
			return
		}
		delay = min(delay*2, time.Minute)
	}
}

// watchCredentials re-reads whoami periodically so an expiring service
// credential is noticed before it stops working.
func (a *App) watchCredentials(ctx context.Context) {
	for {
		if clock.Sleep(ctx, a.clk, 10*time.Minute) != nil {
			return
		}
		if !a.preflightOK.Load() {
			continue
		}
		for label, c := range map[string]*agwp.Client{"orchestrator": a.orch, "reviewer": a.reviewer, "advisor": a.advisorClient} {
			p, err := c.Whoami(ctx)
			if err != nil {
				a.log.Warn("credential check failed", "label", label, "err", err)
				continue
			}
			if !p.ExpiresAt().IsZero() {
				a.metrics.CredentialExpiry.With(label).Set(float64(p.ExpiresAt().Unix()))
				if left := time.Until(p.ExpiresAt()); left < 24*time.Hour {
					a.log.Error("a service credential expires within a day: mint a new one and restart", "identity", p.Identity, "left", left.Round(time.Minute))
				}
			}
		}
	}
}
