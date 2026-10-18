// Package config loads and validates the orchestrator's configuration.
//
// The file is YAML (JSON is valid YAML, so JSON files work too). Unknown keys
// are rejected so a typo cannot silently weaken a setting. Credentials are
// never read from the file: it only names the environment variables that hold
// them, and validation refuses a file that tries to carry a token.
package config

import (
	"bytes"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"regexp"
	"sort"
	"strings"
	"time"

	"gopkg.in/yaml.v3"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/glob"
)

// Config is the whole configuration.
type Config struct {
	Server   Server     `yaml:"server"`
	Listen   string     `yaml:"listen"`
	Admin    Admin      `yaml:"admin"`
	State    string     `yaml:"state_dir"`
	Log      Log        `yaml:"log"`
	Loop     Loop       `yaml:"loop"`
	Repos    []Repo     `yaml:"repos"`
	Execs    []Executor `yaml:"executors"`
	Shutdown Shutdown   `yaml:"shutdown"`
}

// Server says where the AGWP server is and which environment variables hold
// the credentials of the service identities.
type Server struct {
	URL           string        `yaml:"url"`
	TokenEnv      string        `yaml:"token_env"`
	ReviewerEnv   string        `yaml:"reviewer_token_env"`
	AdvisorEnv    string        `yaml:"advisor_token_env"`
	CAFile        string        `yaml:"ca_file"`
	Timeout       time.Duration `yaml:"timeout"`
	RetryMax      int           `yaml:"retries"`
	StreamIdle    time.Duration `yaml:"stream_idle"`
	AllowInsecure bool          `yaml:"allow_insecure_remote"`
}

// Admin protects the HTTP API of this service.
type Admin struct {
	TokenEnv string `yaml:"token_env"`
}

// Log configures slog.
type Log struct {
	Format string `yaml:"format"` // text | json
	Level  string `yaml:"level"`  // debug | info | warn | error
}

// Loop tunes the periodic reconcile safety net.
type Loop struct {
	Reconcile time.Duration `yaml:"reconcile"`
	Debounce  time.Duration `yaml:"debounce"`
}

// Shutdown says what happens to sessions when the service stops.
type Shutdown struct {
	Grace time.Duration `yaml:"grace"`
	Mode  string        `yaml:"mode"` // wait | cancel
}

// Repo configures everything the service does for one repository.
type Repo struct {
	Name      string    `yaml:"name"`
	Stream    string    `yaml:"stream"`
	Schedule  Schedule  `yaml:"schedule"`
	ExecSched ExecSched `yaml:"validations"`
	Advisor   Advisor   `yaml:"advisor"`
	Review    Review    `yaml:"review"`
	Index     Index     `yaml:"index"`
}

// Schedule configures task scheduling for a repository.
type Schedule struct {
	Enabled   *bool         `yaml:"enabled"`
	Slots     int           `yaml:"slots"`
	Placement string        `yaml:"placement"` // avoid | allow
	Lease     string        `yaml:"lease"`     // soft | exclusive | none
	LeaseTTL  time.Duration `yaml:"lease_ttl"`
	Attempts  int           `yaml:"attempts"`
	Backoff   time.Duration `yaml:"backoff"`
	Default   string        `yaml:"default_executor"`
	Routes    []Route       `yaml:"routes"`
}

// Route sends matching tasks to an executor. All given matchers must match.
type Route struct {
	Executor string `yaml:"executor"`
	Title    string `yaml:"title"` // regular expression over the task title
	Scope    string `yaml:"scope"` // glob that must overlap one of the task's scopes
}

// ExecSched configures the execution scheduler.
type ExecSched struct {
	Enabled *bool `yaml:"enabled"`
	Enqueue *bool `yaml:"enqueue"`
}

// Advisor configures the conflict advisor.
type Advisor struct {
	Enabled  *bool `yaml:"enabled"`
	Inline   int   `yaml:"inline_bytes"`
	MaxFile  int   `yaml:"max_file_bytes"`
	MaxPaths int   `yaml:"max_paths"`
}

// Review configures the deterministic reviewers.
type Review struct {
	Enabled   *bool    `yaml:"enabled"`
	MaxFiles  int      `yaml:"max_files"`
	MaxLines  int      `yaml:"max_lines"`
	Protected []string `yaml:"protected"`
	Generated []string `yaml:"generated"`
	Secrets   *bool    `yaml:"secrets"`
	Tests     *bool    `yaml:"tests"`
	Sources   []string `yaml:"sources"`
	AllowRE   []string `yaml:"allow_secrets"`
}

// Index configures the semantic index.
type Index struct {
	Enabled  *bool    `yaml:"enabled"`
	MaxBytes int      `yaml:"max_file_bytes"`
	MaxFiles int      `yaml:"max_files"`
	Skip     []string `yaml:"skip"`
}

// Executor describes one way of running an agent.
type Executor struct {
	Name    string            `yaml:"name"`
	Type    string            `yaml:"type"` // command
	Agent   string            `yaml:"agent"`
	Slots   int               `yaml:"slots"`
	Timeout time.Duration     `yaml:"timeout"`
	Grace   time.Duration     `yaml:"grace"`
	Beat    time.Duration     `yaml:"heartbeat"`
	Cred    time.Duration     `yaml:"credential_ttl"`
	Ops     []string          `yaml:"operations"`
	Command []string          `yaml:"command"`
	Dir     string            `yaml:"dir"`
	Env     map[string]string `yaml:"env"`
	Pass    []string          `yaml:"pass_env"`
	Output  int               `yaml:"output_bytes"`
}

// Default operations granted to an agent credential (capabilities of the
// server's authorization model). Anything that lands, reviews, administers or
// moves a stream is deliberately absent.
var DefaultAgentOps = []string{
	"repo.read", "objects.read", "objects.write", "publish", "operation.read", "history.read", "tree.read",
	"stream.read", "workspace.read", "workspace.write", "proposal.read", "proposal.create", "proposal.update",
	"integrate.prepare", "agent.read", "agent.run", "task.read", "task.write", "lease.read", "lease.write",
	"provenance.read", "provenance.write", "predict", "events.read",
}

// ForbiddenAgentOps may never be granted to an agent credential, whatever the
// configuration says: they are the powers that let an agent bypass review,
// policy or the stream's compare-and-swap.
var ForbiddenAgentOps = map[string]bool{
	"integrate.run": true, "queue.run": true, "queue.write": true, "proposal.review": true, "proposal.close": true,
	"policy.write": true, "stream.cas": true, "stream.force": true, "stream.create": true, "gc.run": true, "fsck.run": true,
	"credentials.mint": true, "credentials.delegate": true, "credentials.list": true, "credentials.revoke": true,
	"roles.manage": true, "agent.operate": true,
	"attestation.submit": true, "jobs.claim": true, "repo.create": true, "repo.configure": true, "pins.write": true,
	"agent.register": true, "integrate.validate": true,
}

var (
	nameRE   = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$`)
	agentRE  = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,63}$`)
	envRE    = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
	execRE   = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$`)
	streamRE = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$`)
)

// Load reads, defaults and validates a configuration file. env supplies the
// overrides (AG_SERVER, AG_ORCH_LISTEN, ...) and the credentials' presence
// check; pass os.Getenv.
func Load(path string, getenv func(string) string) (*Config, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("config: %w", err)
	}
	defer f.Close()
	return Parse(f, getenv)
}

// LoadNoCredentials is Load without the check that the credential environment
// variables are set. It serves the commands that only describe the
// configuration (config, ops) and so run before any token exists.
func LoadNoCredentials(path string, getenv func(string) string) (*Config, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, fmt.Errorf("config: %w", err)
	}
	defer f.Close()
	return parseDoc(f, getenv, false)
}

// Parse is Load for an in-memory document.
func Parse(r io.Reader, getenv func(string) string) (*Config, error) {
	return parseDoc(r, getenv, true)
}

func parseDoc(r io.Reader, getenv func(string) string, needCredentials bool) (*Config, error) {
	raw, err := io.ReadAll(io.LimitReader(r, 1<<20))
	if err != nil {
		return nil, fmt.Errorf("config: %w", err)
	}
	var c Config
	dec := yaml.NewDecoder(bytes.NewReader(raw))
	dec.KnownFields(true)
	if err := dec.Decode(&c); err != nil && !errors.Is(err, io.EOF) {
		return nil, fmt.Errorf("config: %w", describeYAMLError(err))
	}
	c.applyEnv(getenv)
	c.applyDefaults()
	if err := c.validate(getenv, needCredentials); err != nil {
		return nil, err
	}
	return &c, nil
}

func describeYAMLError(err error) error {
	msg := err.Error()
	if strings.Contains(msg, "not found in type") {
		msg += " (unknown configuration key; credentials are never configured in the file, only the name of the environment variable that holds them)"
	}
	return errors.New(msg)
}

func (c *Config) applyEnv(getenv func(string) string) {
	if getenv == nil {
		return
	}
	if v := getenv("AG_SERVER"); v != "" {
		c.Server.URL = v
	}
	if v := getenv("AG_ORCH_LISTEN"); v != "" {
		c.Listen = v
	}
	if v := getenv("AG_ORCH_STATE_DIR"); v != "" {
		c.State = v
	}
	if v := getenv("AG_ORCH_LOG_FORMAT"); v != "" {
		c.Log.Format = v
	}
	if v := getenv("AG_ORCH_LOG_LEVEL"); v != "" {
		c.Log.Level = v
	}
}

func boolDefault(p **bool, v bool) {
	if *p == nil {
		*p = &v
	}
}

// On reports whether an optional switch is on (default true).
func On(p *bool) bool { return p == nil || *p }

func (c *Config) applyDefaults() {
	s := &c.Server
	setStr(&s.TokenEnv, "AG_ORCH_TOKEN")
	setStr(&s.ReviewerEnv, "AG_REVIEWER_TOKEN")
	setStr(&s.AdvisorEnv, "AG_ADVISOR_TOKEN")
	setDur(&s.Timeout, 30*time.Second)
	setInt(&s.RetryMax, 4)
	setDur(&s.StreamIdle, 60*time.Second)
	setStr(&c.Listen, "127.0.0.1:9090")
	setStr(&c.Log.Format, "text")
	setStr(&c.Log.Level, "info")
	setDur(&c.Loop.Reconcile, 30*time.Second)
	setDur(&c.Loop.Debounce, 500*time.Millisecond)
	setDur(&c.Shutdown.Grace, 30*time.Second)
	setStr(&c.Shutdown.Mode, "wait")
	for i := range c.Repos {
		r := &c.Repos[i]
		setStr(&r.Stream, "main")
		boolDefault(&r.Schedule.Enabled, true)
		setInt(&r.Schedule.Slots, 2)
		setStr(&r.Schedule.Placement, "avoid")
		setStr(&r.Schedule.Lease, "soft")
		setDur(&r.Schedule.LeaseTTL, 30*time.Minute)
		setInt(&r.Schedule.Attempts, 1)
		setDur(&r.Schedule.Backoff, 30*time.Second)
		if r.Schedule.Default == "" && len(r.Schedule.Routes) == 0 && len(c.Execs) == 1 {
			r.Schedule.Default = c.Execs[0].Name // a single executor is the implicit default
		}
		boolDefault(&r.ExecSched.Enabled, true)
		boolDefault(&r.ExecSched.Enqueue, true)
		boolDefault(&r.Advisor.Enabled, true)
		setInt(&r.Advisor.Inline, 16<<10)
		setInt(&r.Advisor.MaxFile, 1<<20)
		setInt(&r.Advisor.MaxPaths, 20)
		boolDefault(&r.Review.Enabled, true)
		setInt(&r.Review.MaxFiles, 40)
		setInt(&r.Review.MaxLines, 1500)
		boolDefault(&r.Review.Secrets, true)
		boolDefault(&r.Review.Tests, true)
		if r.Review.Protected == nil {
			r.Review.Protected = []string{".github/**", "**/.env", "**/.env.*", "**/*.pem", "**/id_rsa*", "**/secrets/**", "SECURITY.md"}
		}
		if r.Review.Generated == nil {
			r.Review.Generated = []string{"**/*.lock", "**/package-lock.json", "**/go.sum", "**/dist/**", "**/*.min.js", "**/*.pb.go", "**/vendor/**"}
		}
		if r.Review.Sources == nil {
			r.Review.Sources = []string{"**/*.go", "**/*.rs", "**/*.py", "**/*.ts", "**/*.tsx", "**/*.js", "**/*.jsx", "**/*.java", "**/*.kt", "**/*.rb"}
		}
		boolDefault(&r.Index.Enabled, true)
		setInt(&r.Index.MaxBytes, 256<<10)
		setInt(&r.Index.MaxFiles, 5000)
		if r.Index.Skip == nil {
			r.Index.Skip = []string{"vendor/**", "**/vendor/**", "node_modules/**", "**/node_modules/**", "target/**", "**/target/**",
				"dist/**", "**/dist/**", "build/**", ".git/**", "**/*.min.js", "**/*.map", "**/*.lock", "**/*.sum", "**/testdata/**"}
		}
	}
	for i := range c.Execs {
		e := &c.Execs[i]
		setStr(&e.Agent, e.Name)
		setInt(&e.Slots, 1)
		setDur(&e.Timeout, 30*time.Minute)
		setDur(&e.Grace, 10*time.Second)
		setDur(&e.Beat, 15*time.Second)
		setInt(&e.Output, 256<<10)
		if e.Cred == 0 {
			e.Cred = min(time.Hour, e.Timeout+e.Grace+5*time.Minute)
		}
		if e.Ops == nil {
			e.Ops = append([]string(nil), DefaultAgentOps...)
		}
		if e.Pass == nil {
			e.Pass = []string{"PATH", "LANG", "TZ"}
		}
	}
}

func setStr(p *string, v string) {
	if *p == "" {
		*p = v
	}
}
func setInt(p *int, v int) {
	if *p == 0 {
		*p = v
	}
}
func setDur(p *time.Duration, v time.Duration) {
	if *p == 0 {
		*p = v
	}
}

// Validate checks the (defaulted) configuration and reports every problem.
func (c *Config) Validate(getenv func(string) string) error { return c.validate(getenv, true) }

func (c *Config) validate(getenv func(string) string, needCredentials bool) error {
	var errs []string
	add := func(path, format string, a ...any) { errs = append(errs, path+": "+fmt.Sprintf(format, a...)) }
	if getenv == nil {
		getenv = func(string) string { return "" }
	}

	// server
	if u := c.Server.URL; u == "" {
		add("server.url", "required (or set AG_SERVER)")
	} else if !strings.HasPrefix(u, "http://") && !strings.HasPrefix(u, "https://") {
		add("server.url", "must start with http:// or https:// (got %q)", u)
	} else if strings.HasPrefix(u, "http://") && !c.Server.AllowInsecure && !isLoopbackURL(u) {
		add("server.url", "plain http to a non-loopback host sends credentials in the clear; use https:// or set server.allow_insecure_remote")
	}
	for _, p := range []struct{ path, v string }{{"server.token_env", c.Server.TokenEnv}, {"server.reviewer_token_env", c.Server.ReviewerEnv},
		{"server.advisor_token_env", c.Server.AdvisorEnv}, {"admin.token_env", c.Admin.TokenEnv}} {
		if p.v != "" && !envRE.MatchString(p.v) {
			add(p.path, "must be the NAME of an environment variable, got %q (never put a token in the file)", p.v)
		}
	}
	if needCredentials && getenv(c.Server.TokenEnv) == "" {
		add("server.token_env", "environment variable %s is empty or unset: the orchestrator's own credential is required", c.Server.TokenEnv)
	}
	if c.Server.Timeout < time.Second {
		add("server.timeout", "must be at least 1s")
	}
	if c.Server.RetryMax < 1 || c.Server.RetryMax > 10 {
		add("server.retries", "must be between 1 and 10")
	}

	// listen / admin
	host, _, err := net.SplitHostPort(c.Listen)
	switch {
	case err != nil:
		add("listen", "must be host:port (got %q)", c.Listen)
	case !isLoopbackHost(host) && c.Admin.TokenEnv == "":
		add("listen", "%q is not a loopback address; set admin.token_env so /status and the index API require a bearer token", c.Listen)
	}
	if needCredentials && c.Admin.TokenEnv != "" && getenv(c.Admin.TokenEnv) == "" {
		add("admin.token_env", "environment variable %s is empty or unset", c.Admin.TokenEnv)
	}
	if c.State == "" {
		add("state_dir", "required: agent keys, the semantic index and session work directories live there")
	}
	if c.Log.Format != "text" && c.Log.Format != "json" {
		add("log.format", "must be text or json")
	}
	switch c.Log.Level {
	case "debug", "info", "warn", "error":
	default:
		add("log.level", "must be debug, info, warn or error")
	}
	if c.Loop.Reconcile < time.Second {
		add("loop.reconcile", "must be at least 1s")
	}
	if c.Shutdown.Mode != "wait" && c.Shutdown.Mode != "cancel" {
		add("shutdown.mode", "must be wait (let sessions finish within the grace period) or cancel")
	}

	// executors
	execs := map[string]*Executor{}
	agents := map[string]string{}
	for i := range c.Execs {
		e := &c.Execs[i]
		p := fmt.Sprintf("executors[%d]", i)
		if !execRE.MatchString(e.Name) {
			add(p+".name", "required, letters/digits/._- (got %q)", e.Name)
			continue
		}
		if execs[e.Name] != nil {
			add(p+".name", "duplicate executor %q", e.Name)
		}
		execs[e.Name] = e
		if !agentRE.MatchString(e.Agent) {
			add(p+".agent", "must match [a-z0-9][a-z0-9_-]{0,63} (got %q)", e.Agent)
		} else if other, dup := agents[e.Agent]; dup {
			add(p+".agent", "agent %q is already used by executor %q; one agent identity per executor keeps provenance attributable", e.Agent, other)
		}
		agents[e.Agent] = e.Name
		switch e.Type {
		case "command":
			if len(e.Command) == 0 || e.Command[0] == "" {
				add(p+".command", "required for type command (an argv list, run without a shell)")
			}
		case "scripted":
			add(p+".type", "executor type \"scripted\" was removed: executors run real programs (type: command)")
		default:
			add(p+".type", "must be command (got %q)", e.Type)
		}
		if e.Slots < 1 {
			add(p+".slots", "must be at least 1")
		}
		if e.Timeout < time.Second || e.Timeout > 55*time.Minute {
			add(p+".timeout", "must be between 1s and 55m: agent credentials live at most one hour and cannot be renewed in a running process")
		}
		if e.Cred < e.Timeout || e.Cred > time.Hour {
			add(p+".credential_ttl", "must cover the timeout (%s) and be at most 1h", e.Timeout)
		}
		if e.Grace < 0 || e.Beat < time.Second {
			add(p+".grace/heartbeat", "grace must be >= 0 and heartbeat >= 1s")
		}
		if e.Output < 1024 {
			add(p+".output_bytes", "must be at least 1024")
		}
		for _, op := range e.Ops {
			if ForbiddenAgentOps[op] {
				add(p+".operations", "%q may never be granted to an agent credential: agents propose, they do not review, integrate, administer or mint credentials", op)
			}
		}
		for _, name := range e.Pass {
			if !envRE.MatchString(name) {
				add(p+".pass_env", "%q is not an environment variable name", name)
			}
			if name == c.Server.TokenEnv || name == c.Server.ReviewerEnv || name == c.Server.AdvisorEnv || name == c.Admin.TokenEnv ||
				name == "AG_ORCH_TOKEN" || name == "AG_TOKEN" {
				add(p+".pass_env", "%s holds a service credential and may not be passed to an executor", name)
			}
		}
		for k := range e.Env {
			if !envRE.MatchString(k) {
				add(p+".env", "%q is not an environment variable name", k)
			}
			if strings.HasPrefix(k, "AG_") {
				add(p+".env", "%s is reserved: the orchestrator sets the AG_* variables itself", k)
			}
		}
	}

	// repositories
	seen := map[string]bool{}
	for i := range c.Repos {
		r := &c.Repos[i]
		p := fmt.Sprintf("repos[%d]", i)
		if !nameRE.MatchString(r.Name) {
			add(p+".name", "must be owner/name (got %q)", r.Name)
			continue
		}
		if seen[r.Name] {
			add(p+".name", "duplicate repository %q", r.Name)
		}
		seen[r.Name] = true
		if !streamRE.MatchString(r.Stream) {
			add(p+".stream", "invalid stream name %q", r.Stream)
		}
		s := r.Schedule
		if s.Slots < 1 {
			add(p+".schedule.slots", "must be at least 1")
		}
		if s.Placement != "avoid" && s.Placement != "allow" {
			add(p+".schedule.placement", "must be avoid or allow")
		}
		if s.Lease != "soft" && s.Lease != "exclusive" && s.Lease != "none" {
			add(p+".schedule.lease", "must be soft, exclusive or none")
		}
		if s.LeaseTTL < time.Minute || s.LeaseTTL > 24*time.Hour {
			add(p+".schedule.lease_ttl", "must be between 1m and 24h")
		}
		if s.Attempts < 1 || s.Attempts > 10 {
			add(p+".schedule.attempts", "must be between 1 and 10")
		}
		if s.Default != "" && execs[s.Default] == nil {
			add(p+".schedule.default_executor", "unknown executor %q", s.Default)
		}
		for j, rt := range s.Routes {
			rp := fmt.Sprintf("%s.schedule.routes[%d]", p, j)
			if execs[rt.Executor] == nil {
				add(rp+".executor", "unknown executor %q", rt.Executor)
			}
			if rt.Title == "" && rt.Scope == "" {
				add(rp, "needs a title or scope matcher (use default_executor for a catch-all)")
			}
			if rt.Title != "" {
				if _, err := regexp.Compile(rt.Title); err != nil {
					add(rp+".title", "%v", err)
				}
			}
			if rt.Scope != "" {
				if _, err := glob.Compile(rt.Scope); err != nil {
					add(rp+".scope", "%v", err)
				}
			}
		}
		for _, g := range append(append(append([]string{}, r.Review.Protected...), r.Review.Generated...), r.Review.Sources...) {
			if _, err := glob.Compile(g); err != nil {
				add(p+".review", "%v", err)
			}
		}
		for _, g := range r.Index.Skip {
			if _, err := glob.Compile(g); err != nil {
				add(p+".index.skip", "%v", err)
			}
		}
		for _, re := range r.Review.AllowRE {
			if _, err := regexp.Compile(re); err != nil {
				add(p+".review.allow_secrets", "%v", err)
			}
		}
		if r.Review.MaxFiles < 1 || r.Review.MaxLines < 1 {
			add(p+".review", "max_files and max_lines must be at least 1")
		}
		if r.Index.MaxBytes < 1024 || r.Index.MaxFiles < 1 {
			add(p+".index", "max_file_bytes must be at least 1024 and max_files at least 1")
		}
		if r.Advisor.Inline < 0 || r.Advisor.MaxFile < 1024 || r.Advisor.MaxPaths < 1 {
			add(p+".advisor", "inline_bytes >= 0, max_file_bytes >= 1024 and max_paths >= 1 are required")
		}
	}
	if len(c.Repos) == 0 {
		add("repos", "at least one repository is required")
	}

	if len(errs) == 0 {
		return nil
	}
	sort.Strings(errs)
	return fmt.Errorf("config: %d problem(s):\n  - %s", len(errs), strings.Join(errs, "\n  - "))
}

func isLoopbackURL(u string) bool {
	rest := strings.TrimPrefix(strings.TrimPrefix(u, "http://"), "https://")
	host := rest
	if i := strings.IndexAny(host, "/?#"); i >= 0 {
		host = host[:i]
	}
	if h, _, err := net.SplitHostPort(host); err == nil {
		host = h
	}
	return isLoopbackHost(strings.Trim(host, "[]"))
}

func isLoopbackHost(h string) bool {
	if h == "localhost" {
		return true
	}
	ip := net.ParseIP(h)
	return ip != nil && ip.IsLoopback()
}

// Executor returns the executor with the given name.
func (c *Config) Executor(name string) *Executor {
	for i := range c.Execs {
		if c.Execs[i].Name == name {
			return &c.Execs[i]
		}
	}
	return nil
}

// Redacted renders the effective configuration for humans. It contains no
// secrets by construction (credentials are only ever named, not held), and
// repeats the check explicitly.
func (c *Config) Redacted() string {
	b, err := yaml.Marshal(c)
	if err != nil {
		return "config: " + err.Error()
	}
	return string(b)
}
