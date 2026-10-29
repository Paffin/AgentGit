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
	"net/url"
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
	// LLM configures the language-model client used by the components whose mode is "model". Nothing in it is a
	// default of any vendor: the endpoint, the model and the NAME of the variable that holds the key are yours.
	LLM LLM `yaml:"llm"`
}

// Modes of the advisor, the reviewers and the semantic index.
const (
	// ModeHeuristic: deterministic rules and keyword matching. Honest about what it is: not understanding.
	ModeHeuristic = "heuristic"
	// ModeModel: a language model reads the (bounded, fenced) repository text and PROPOSES; the heuristic checks that
	// are safety checks (secret scanning, size, protected paths) still run. A model never integrates anything.
	ModeModel = "model"
)

// LLM is the OpenAI-compatible chat-completions endpoint of the model-backed components.
type LLM struct {
	BaseURL string `yaml:"base_url"` // API root; "/chat/completions" is appended
	Model   string `yaml:"model"`
	// APIKeyEnv is the NAME of the environment variable holding the key. Empty: no Authorization header is sent
	// (right for a gateway or proxy that adds the credential itself, or a local server).
	APIKeyEnv string `yaml:"api_key_env"`
	CAFile    string `yaml:"ca_file"`
	// Retries after the first attempt; nil: 3.
	Timeout         time.Duration     `yaml:"timeout"`
	Retries         *int              `yaml:"retries"`
	MaxOutputTokens int               `yaml:"max_output_tokens"`
	Temperature     *float64          `yaml:"temperature"` // nil: 0
	MaxInputBytes   int               `yaml:"max_input_bytes"`
	JSONMode        bool              `yaml:"json_mode"` // ask for response_format json_object (not every model supports it)
	Concurrency     int               `yaml:"concurrency"`
	Headers         map[string]string `yaml:"headers"` // non-secret extra headers
	Budget          LLMBudget         `yaml:"budget"`
	AllowInsecure   bool              `yaml:"allow_insecure_remote"`
}

// LLMBudget caps what the orchestrator may spend. nil: the default; 0: unlimited. The counters restart with the process.
type LLMBudget struct {
	RequestsPerHour *int `yaml:"requests_per_hour"` // default 200
	TokensPerDay    *int `yaml:"tokens_per_day"`    // default 2,000,000
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
	MaxResponse   int64         `yaml:"max_response_bytes"` // bound on a response body the client reads
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
	// OnPredictError says what placement does when the server's conflict prediction cannot be read: "start"
	// (default) places tasks by the other evidence and says so in /status and the metrics, "defer" holds the
	// tasks back until the prediction is available again.
	OnPredictError string `yaml:"on_predict_error"`
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
	// Refresh is how often a proposal that is not fully green is prepared again when nothing is known to have
	// changed; Recent bounds the integrated proposals kept for /status; JobsLimit bounds the validation jobs read
	// for /status.
	Refresh   time.Duration `yaml:"refresh"`
	Recent    int           `yaml:"recent"`
	JobsLimit int           `yaml:"jobs_limit"`
}

// Advisor configures the conflict advisor.
type Advisor struct {
	Enabled  *bool  `yaml:"enabled"`
	Mode     string `yaml:"mode"` // heuristic (default) | model
	Inline   int    `yaml:"inline_bytes"`
	MaxFile  int    `yaml:"max_file_bytes"`
	MaxPaths int    `yaml:"max_paths"`
	// ModelFileBytes bounds each of the three versions of a file placed in a model prompt (model mode); a larger file
	// is left to the rules.
	ModelFileBytes int `yaml:"model_file_bytes"`
}

// Review configures the deterministic reviewers.
type Review struct {
	Enabled   *bool    `yaml:"enabled"`
	Mode      string   `yaml:"mode"` // heuristic (default) | model: the model adds a reading of the diff to the checks below
	MaxFiles  int      `yaml:"max_files"`
	MaxLines  int      `yaml:"max_lines"`
	Protected []string `yaml:"protected"`
	Generated []string `yaml:"generated"`
	Secrets   *bool    `yaml:"secrets"`
	Tests     *bool    `yaml:"tests"`
	Sources   []string `yaml:"sources"`
	AllowRE   []string `yaml:"allow_secrets"`
	// An initial import (a proposal into an empty stream) has no diff to read: its files are fetched one by one.
	// ImportFiles bounds how many are fetched, ImportBytes how large one may be; whatever is beyond the bounds is
	// reported as not inspected.
	ImportFiles int `yaml:"import_files"`
	ImportBytes int `yaml:"import_file_bytes"`
	// Model mode: ModelDiffBytes bounds the patch text placed in the prompt, ModelMaxFindings the findings taken from one answer.
	ModelDiffBytes   int `yaml:"model_diff_bytes"`
	ModelMaxFindings int `yaml:"model_max_findings"`
}

// Index configures the semantic index.
type Index struct {
	Enabled  *bool    `yaml:"enabled"`
	Mode     string   `yaml:"mode"` // heuristic (default) | model: the model judges the overlaps the index finds
	MaxBytes int      `yaml:"max_file_bytes"`
	MaxFiles int      `yaml:"max_files"`
	Skip     []string `yaml:"skip"`
	// Analysing the definitions an open proposal changes is bounded too: the files of one proposal that are read
	// (each at most MaxBytes), and the proposals analysed in one pass. What the bounds cut is reported as partial.
	// Model mode: JudgePatchBytes bounds the diff text placed in a prompt, JudgePerPass the pairs judged in one pass.
	JudgePatchBytes  int `yaml:"judge_patch_bytes"`
	JudgePerPass     int `yaml:"judge_pairs_per_pass"`
	ProposalFiles    int `yaml:"proposal_max_files"`
	ProposalsPerPass int `yaml:"proposals_per_pass"`
}

// Executor describes one way of running an agent.
type Executor struct {
	// PublishOutput opts in to saving a bounded redacted tail on the AgentGit server.
	PublishOutput bool `yaml:"publish_output"`
	// OutputInterval optionally publishes live snapshots; zero keeps final-only capture.
	OutputInterval time.Duration     `yaml:"output_interval"`
	Name           string            `yaml:"name"`
	Type           string            `yaml:"type"` // command
	Agent          string            `yaml:"agent"`
	Slots          int               `yaml:"slots"`
	Timeout        time.Duration     `yaml:"timeout"`
	Grace          time.Duration     `yaml:"grace"`
	Beat           time.Duration     `yaml:"heartbeat"`
	Cred           time.Duration     `yaml:"credential_ttl"`
	Ops            []string          `yaml:"operations"`
	Command        []string          `yaml:"command"`
	Dir            string            `yaml:"dir"`
	Env            map[string]string `yaml:"env"`
	Pass           []string          `yaml:"pass_env"`
	Output         int               `yaml:"output_bytes"`
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
	nameRE       = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]*/[A-Za-z0-9][A-Za-z0-9._-]*$`)
	agentRE      = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,63}$`)
	envRE        = regexp.MustCompile(`^[A-Za-z_][A-Za-z0-9_]*$`)
	httpHeaderRE = regexp.MustCompile(`^[A-Za-z0-9-]+$`)
	execRE       = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$`)
	streamRE     = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._/-]{0,127}$`)
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
	if s.MaxResponse == 0 {
		s.MaxResponse = 64 << 20
	}
	setStr(&c.Listen, "127.0.0.1:9090")
	setStr(&c.Log.Format, "text")
	setStr(&c.Log.Level, "info")
	setDur(&c.Loop.Reconcile, 30*time.Second)
	setDur(&c.Loop.Debounce, 500*time.Millisecond)
	setDur(&c.Shutdown.Grace, 30*time.Second)
	setStr(&c.Shutdown.Mode, "wait")
	l := &c.LLM
	setDur(&l.Timeout, 60*time.Second)
	setInt(&l.MaxOutputTokens, 1500)
	setInt(&l.MaxInputBytes, 120_000)
	setInt(&l.Concurrency, 2)
	for i := range c.Repos {
		r := &c.Repos[i]
		setStr(&r.Stream, "main")
		boolDefault(&r.Schedule.Enabled, true)
		setInt(&r.Schedule.Slots, 2)
		setStr(&r.Schedule.Placement, "avoid")
		setStr(&r.Schedule.OnPredictError, "start")
		setStr(&r.Advisor.Mode, ModeHeuristic)
		setStr(&r.Review.Mode, ModeHeuristic)
		setStr(&r.Index.Mode, ModeHeuristic)
		setStr(&r.Schedule.Lease, "soft")
		setDur(&r.Schedule.LeaseTTL, 30*time.Minute)
		setInt(&r.Schedule.Attempts, 1)
		setDur(&r.Schedule.Backoff, 30*time.Second)
		if r.Schedule.Default == "" && len(r.Schedule.Routes) == 0 && len(c.Execs) == 1 {
			r.Schedule.Default = c.Execs[0].Name // a single executor is the implicit default
		}
		boolDefault(&r.ExecSched.Enabled, true)
		boolDefault(&r.ExecSched.Enqueue, true)
		setDur(&r.ExecSched.Refresh, 15*time.Second)
		setInt(&r.ExecSched.Recent, 20)
		setInt(&r.ExecSched.JobsLimit, 200)
		boolDefault(&r.Advisor.Enabled, true)
		setInt(&r.Advisor.Inline, 16<<10)
		setInt(&r.Advisor.MaxFile, 1<<20)
		setInt(&r.Advisor.MaxPaths, 20)
		setInt(&r.Advisor.ModelFileBytes, 24_000)
		setInt(&r.Review.ModelDiffBytes, 40_000)
		setInt(&r.Review.ModelMaxFindings, 8)
		setInt(&r.Index.JudgePatchBytes, 30_000)
		setInt(&r.Index.JudgePerPass, 3)
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
		setInt(&r.Index.ProposalFiles, 30)
		setInt(&r.Index.ProposalsPerPass, 10)
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
		if p.v == "" {
			continue
		}
		// A value that looks like a credential is never echoed back: neither here nor in a later "unset" message.
		switch {
		case LooksLikeCredential(p.v):
			add(p.path, "must be the NAME of an environment variable, not a credential (this value looks like one and is not shown): put the token in the environment and name the variable here")
		case !envRE.MatchString(p.v):
			add(p.path, "must be the NAME of an environment variable, got %q (never put a token in the file)", p.v)
		}
	}
	if needCredentials && !LooksLikeCredential(c.Server.TokenEnv) && getenv(c.Server.TokenEnv) == "" {
		add("server.token_env", "environment variable %s is empty or unset: the orchestrator's own credential is required", c.Server.TokenEnv)
	}
	if c.Server.Timeout < time.Second {
		add("server.timeout", "must be at least 1s")
	}
	if c.Server.RetryMax < 1 || c.Server.RetryMax > 10 {
		add("server.retries", "must be between 1 and 10")
	}
	if c.Server.MaxResponse < 1<<10 || c.Server.MaxResponse > 1<<30 {
		add("server.max_response_bytes", "must be between 1 KiB and 1 GiB")
	}

	// listen / admin
	host, _, err := net.SplitHostPort(c.Listen)
	switch {
	case err != nil:
		add("listen", "must be host:port (got %q)", c.Listen)
	case !isLoopbackHost(host) && c.Admin.TokenEnv == "":
		add("listen", "%q is not a loopback address; set admin.token_env so /status and the index API require a bearer token", c.Listen)
	}
	if needCredentials && c.Admin.TokenEnv != "" && !LooksLikeCredential(c.Admin.TokenEnv) && getenv(c.Admin.TokenEnv) == "" {
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
		if e.OutputInterval != 0 && (!e.PublishOutput || e.OutputInterval < time.Second || e.OutputInterval > time.Minute) {
			add(p+".output_interval", "requires publish_output and must be between 1s and 1m, or 0 for final-only capture")
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
			if LooksLikeCredential(name) {
				add(p+".pass_env", "an entry looks like a credential, not a variable name (not shown)")
				continue
			}
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

	c.validateLLM(add, getenv, needCredentials)

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
		if s.OnPredictError != "start" && s.OnPredictError != "defer" {
			add(p+".schedule.on_predict_error", "must be start or defer")
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
		if r.ExecSched.Refresh < time.Second || r.ExecSched.Recent < 1 || r.ExecSched.JobsLimit < 1 {
			add(p+".validations", "refresh must be at least 1s, recent and jobs_limit at least 1")
		}
		if r.Index.ProposalFiles < 1 || r.Index.ProposalsPerPass < 1 {
			add(p+".index", "proposal_max_files and proposals_per_pass must be at least 1")
		}
		if r.Advisor.Inline < 0 || r.Advisor.MaxFile < 1024 || r.Advisor.MaxPaths < 1 {
			add(p+".advisor", "inline_bytes >= 0, max_file_bytes >= 1024 and max_paths >= 1 are required")
		}
		if r.Advisor.ModelFileBytes < 1024 || r.Review.ModelDiffBytes < 1024 || r.Review.ModelMaxFindings < 1 || r.Index.JudgePatchBytes < 1024 || r.Index.JudgePerPass < 1 {
			add(p, "model_file_bytes, model_diff_bytes and judge_patch_bytes must be at least 1024; model_max_findings and judge_pairs_per_pass at least 1")
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

// LooksLikeCredential reports whether a string that should be the NAME of an environment variable is more likely
// the secret itself: it carries the prefix of an AgentGit credential, or is longer than any variable name in use.
// Such a value is never echoed back in an error message or in the effective configuration.
func LooksLikeCredential(v string) bool {
	if len(v) > 64 {
		return true
	}
	for _, p := range []string{"agt_", "sk-", "sk_", "pk-", "ghp_", "gho_", "ghs_", "github_pat_", "xoxb-", "xoxp-", "AKIA", "Bearer "} {
		if strings.HasPrefix(v, p) {
			return true
		}
	}
	return false
}

// RedactedValue replaces the values of executor environments in the effective configuration.
const RedactedValue = "<redacted>"

// RetryCount is the number of retries after the first attempt.
func (l LLM) RetryCount() int {
	if l.Retries == nil {
		return 3
	}
	return *l.Retries
}

// Temp is the sampling temperature (default 0).
func (l LLM) Temp() float64 {
	if l.Temperature == nil {
		return 0
	}
	return *l.Temperature
}

// RequestsPerHourLimit is the request budget per hour (0: unlimited).
func (l LLM) RequestsPerHourLimit() int {
	if l.Budget.RequestsPerHour == nil {
		return 200
	}
	return *l.Budget.RequestsPerHour
}

// TokensPerDayLimit is the token budget per day (0: unlimited).
func (l LLM) TokensPerDayLimit() int {
	if l.Budget.TokensPerDay == nil {
		return 2_000_000
	}
	return *l.Budget.TokensPerDay
}

// UsesModel reports whether any component of any repository runs in model mode.
func (c *Config) UsesModel() bool {
	for _, r := range c.Repos {
		if r.Advisor.Mode == ModeModel || r.Review.Mode == ModeModel || r.Index.Mode == ModeModel {
			return true
		}
	}
	return false
}

func (c *Config) validateLLM(add func(path, format string, a ...any), getenv func(string) string, needCredentials bool) {
	for i, r := range c.Repos {
		for _, m := range []struct{ path, mode string }{{"advisor", r.Advisor.Mode}, {"review", r.Review.Mode}, {"index", r.Index.Mode}} {
			if m.mode != "" && m.mode != ModeHeuristic && m.mode != ModeModel {
				add(fmt.Sprintf("repos[%d].%s.mode", i, m.path), "must be %s or %s (got %q)", ModeHeuristic, ModeModel, m.mode)
			}
		}
	}
	l := c.LLM
	if l.Timeout < time.Second || l.MaxOutputTokens < 16 || l.MaxInputBytes < 1024 || l.Concurrency < 1 || l.Concurrency > 16 {
		add("llm", "timeout must be at least 1s, max_output_tokens at least 16, max_input_bytes at least 1024 and concurrency between 1 and 16")
	}
	if r := l.RetryCount(); r < 0 || r > 8 {
		add("llm.retries", "must be between 0 and 8")
	}
	if t := l.Temp(); t < 0 || t > 2 {
		add("llm.temperature", "must be between 0 and 2")
	}
	if l.RequestsPerHourLimit() < 0 || l.TokensPerDayLimit() < 0 {
		add("llm.budget", "limits cannot be negative (0 means unlimited)")
	}
	if !c.UsesModel() {
		return
	}
	switch {
	case l.BaseURL == "":
		add("llm.base_url", "required when a component runs in model mode")
	case !strings.HasPrefix(l.BaseURL, "http://") && !strings.HasPrefix(l.BaseURL, "https://"):
		add("llm.base_url", "must start with http:// or https:// (got %q)", l.BaseURL)
	case baseURLHasSecretSpot(l.BaseURL):
		add("llm.base_url", "must not contain credentials or a query string (the key goes in the environment variable named by api_key_env)")
	case strings.HasPrefix(l.BaseURL, "http://") && !l.AllowInsecure && !isLoopbackURL(l.BaseURL):
		add("llm.base_url", "plain http to a non-loopback host would send the API key in the clear; use https:// or set llm.allow_insecure_remote")
	}
	if strings.TrimSpace(l.Model) == "" {
		add("llm.model", "required when a component runs in model mode (there is no default: the model is your choice)")
	}
	switch {
	case l.APIKeyEnv == "":
	case LooksLikeCredential(l.APIKeyEnv):
		add("llm.api_key_env", "must be the NAME of an environment variable, not a key (this value looks like one and is not shown)")
	case !envRE.MatchString(l.APIKeyEnv):
		add("llm.api_key_env", "must be the NAME of an environment variable (letters, digits and underscores); this value is not shown because it may be a key")
	case needCredentials && getenv(l.APIKeyEnv) == "":
		add("llm.api_key_env", "environment variable %s is empty or unset: the model API key is required (leave api_key_env empty only behind a gateway that adds the credential)", l.APIKeyEnv)
	}
	for k := range l.Headers {
		if !httpHeaderRE.MatchString(k) || strings.EqualFold(k, "authorization") {
			add("llm.headers", "%q is not a usable header name (the Authorization header is set from api_key_env only)", k)
		}
	}
}

// Redacted renders the effective configuration for humans. Credentials are only ever named in it, never held; on
// top of that the values of every executor's `env` (where an operator may paste an API key for the agent) are
// replaced, keeping the variable names.
func (c *Config) Redacted() string {
	cp := *c
	cp.Execs = make([]Executor, len(c.Execs))
	for i, e := range c.Execs {
		if e.Env != nil {
			env := make(map[string]string, len(e.Env))
			for k := range e.Env {
				env[k] = RedactedValue
			}
			e.Env = env
		}
		cp.Execs[i] = e
	}
	b, err := yaml.Marshal(&cp)
	if err != nil {
		return "config: " + err.Error()
	}
	return string(b)
}

// baseURLHasSecretSpot reports a base URL that carries user information or a query: the places a key could be pasted.
func baseURLHasSecretSpot(raw string) bool {
	u, err := url.Parse(raw)
	return err == nil && (u.User != nil || u.RawQuery != "" || u.Fragment != "")
}
