package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func env(m map[string]string) func(string) string {
	return func(k string) string { return m[k] }
}

var goodEnv = map[string]string{"AG_ORCH_TOKEN": "agt_x_secretsecretsecret"}

const minimal = `
server:
  url: http://127.0.0.1:8080
state_dir: /tmp/orch
repos:
  - name: acme/web
executors:
  - name: docs-bot
    type: command
    command: ["/usr/bin/true"]
`

func parse(t *testing.T, doc string, e map[string]string) (*Config, error) {
	t.Helper()
	return Parse(strings.NewReader(doc), env(e))
}

func TestMinimalConfigGetsSafeDefaults(t *testing.T) {
	c, err := parse(t, minimal, goodEnv)
	if err != nil {
		t.Fatal(err)
	}
	r := c.Repos[0]
	if r.Stream != "main" || r.Schedule.Slots != 2 || r.Schedule.Placement != "avoid" || r.Schedule.Lease != "soft" ||
		r.Schedule.Attempts != 1 || !On(r.Review.Enabled) || !On(r.ExecSched.Enqueue) || r.Schedule.Default != "docs-bot" {
		t.Fatalf("repo defaults wrong: %+v", r)
	}
	e := c.Execs[0]
	if e.Agent != "docs-bot" || e.Slots != 1 || e.Timeout != 30*time.Minute || e.Cred != 30*time.Minute+10*time.Second+5*time.Minute {
		t.Fatalf("executor defaults wrong: %+v", e)
	}
	if c.Listen != "127.0.0.1:9090" || c.Loop.Reconcile != 30*time.Second || c.Shutdown.Mode != "wait" {
		t.Fatalf("global defaults wrong: %+v", c)
	}
	for _, op := range e.Ops {
		if ForbiddenAgentOps[op] {
			t.Fatalf("default agent operations include forbidden %q", op)
		}
	}
}

func TestEnvironmentOverridesTheFile(t *testing.T) {
	c, err := parse(t, minimal, map[string]string{
		"AG_ORCH_TOKEN": "x", "AG_SERVER": "http://localhost:9999", "AG_ORCH_LISTEN": "127.0.0.1:1234",
		"AG_ORCH_STATE_DIR": "/var/x", "AG_ORCH_LOG_FORMAT": "json", "AG_ORCH_LOG_LEVEL": "debug",
	})
	if err != nil {
		t.Fatal(err)
	}
	if c.Server.URL != "http://localhost:9999" || c.Listen != "127.0.0.1:1234" || c.State != "/var/x" || c.Log.Format != "json" || c.Log.Level != "debug" {
		t.Fatalf("overrides not applied: %+v", c)
	}
}

func TestLoadNoCredentialsSkipsOnlyThePresenceCheck(t *testing.T) {
	dir := t.TempDir()
	good := filepath.Join(dir, "good.yaml")
	if err := os.WriteFile(good, []byte(minimal), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := Load(good, env(nil)); err == nil || !strings.Contains(err.Error(), "AG_ORCH_TOKEN") {
		t.Fatalf("Load without the credential must fail naming the variable, got %v", err)
	}
	c, err := LoadNoCredentials(good, env(nil))
	if err != nil {
		t.Fatalf("LoadNoCredentials: %v", err)
	}
	if c.Repos[0].Name != "acme/web" {
		t.Fatalf("%+v", c.Repos)
	}
	// everything else is still validated
	bad := filepath.Join(dir, "bad.yaml")
	if err := os.WriteFile(bad, []byte(strings.Replace(minimal, "http://127.0.0.1:8080", "ftp://x", 1)), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadNoCredentials(bad, env(nil)); err == nil || !strings.Contains(err.Error(), "server.url") {
		t.Fatalf("other validation errors must remain, got %v", err)
	}
	if _, err := LoadNoCredentials(filepath.Join(dir, "missing.yaml"), env(nil)); err == nil {
		t.Fatal("a missing file must be an error")
	}
}

func TestJSONIsAcceptedToo(t *testing.T) {
	doc := `{"server":{"url":"http://127.0.0.1:1"},"state_dir":"/s","repos":[{"name":"a/b"}],
	"executors":[{"name":"x","type":"command","command":["true"],"timeout":"5m"}]}`
	c, err := parse(t, doc, goodEnv)
	if err != nil {
		t.Fatal(err)
	}
	if c.Execs[0].Timeout != 5*time.Minute {
		t.Fatalf("timeout = %v", c.Execs[0].Timeout)
	}
}

func TestValidationReportsEveryProblemWithPaths(t *testing.T) {
	doc := `
server: {url: "ftp://nope"}
listen: "not-an-address"
state_dir: ""
log: {format: xml, level: loud}
repos:
  - name: justaname
  - name: acme/web
    schedule: {slots: -1, placement: maybe, lease: forever, attempts: 99, routes: [{executor: ghost, title: "("}]}
executors:
  - {name: a, type: carrier-pigeon, agent: BAD NAME}
  - {name: b, type: command}
  - {name: c, type: scripted, command: [x], agent: dup}
  - {name: d, type: command, command: [x], agent: dup, timeout: 3h}
`
	_, err := parse(t, doc, nil)
	if err == nil {
		t.Fatal("expected errors")
	}
	msg := err.Error()
	for _, want := range []string{
		"server.url", "server.token_env", "listen", "state_dir", "log.format", "log.level", "repos[0].name",
		"repos[1].schedule.slots", "repos[1].schedule.placement", "repos[1].schedule.lease", "repos[1].schedule.attempts",
		"routes[0].executor", "routes[0].title", "executors[0].type", "executors[0].agent", "executors[1].command",
		"executors[2].type", "was removed", "executors[3].agent", "executors[3].timeout",
	} {
		if !strings.Contains(msg, want) {
			t.Errorf("error message does not mention %q:\n%s", want, msg)
		}
	}
}

func TestTokensCannotLiveInTheFile(t *testing.T) {
	doc := strings.Replace(minimal, "url: http://127.0.0.1:8080", "url: http://127.0.0.1:8080\n  token: agt_literal_token_value", 1)
	_, err := parse(t, doc, goodEnv)
	if err == nil || !strings.Contains(err.Error(), "token") || !strings.Contains(err.Error(), "never configured in the file") {
		t.Fatalf("err = %v", err)
	}
}

func TestMissingServiceTokenIsAClearError(t *testing.T) {
	_, err := parse(t, minimal, map[string]string{})
	if err == nil || !strings.Contains(err.Error(), "AG_ORCH_TOKEN") {
		t.Fatalf("err = %v", err)
	}
	if strings.Contains(err.Error(), "secretsecret") {
		t.Fatal("error leaks a secret")
	}
}

func TestTokenEnvMustBeANameNotAToken(t *testing.T) {
	doc := strings.Replace(minimal, "server:\n  url: http://127.0.0.1:8080", "server:\n  url: http://127.0.0.1:8080\n  token_env: agt_abc.def-ghi", 1)
	_, err := parse(t, doc, goodEnv)
	if err == nil || !strings.Contains(err.Error(), "NAME of an environment variable") {
		t.Fatalf("err = %v", err)
	}
}

func TestPlainHTTPToRemoteHostsIsRefused(t *testing.T) {
	doc := strings.Replace(minimal, "http://127.0.0.1:8080", "http://scm.example.com", 1)
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "in the clear") {
		t.Fatalf("err = %v", err)
	}
	for _, ok := range []string{"https://scm.example.com", "http://localhost:8080", "http://[::1]:8080", "http://127.0.0.1"} {
		if _, err := parse(t, strings.Replace(minimal, "http://127.0.0.1:8080", ok, 1), goodEnv); err != nil {
			t.Errorf("%s should be accepted: %v", ok, err)
		}
	}
	doc2 := strings.Replace(minimal, "url: http://127.0.0.1:8080", "url: http://scm.internal\n  allow_insecure_remote: true", 1)
	if _, err := parse(t, doc2, goodEnv); err != nil {
		t.Errorf("explicit opt-in should be accepted: %v", err)
	}
}

func TestNonLoopbackListenNeedsAnAdminToken(t *testing.T) {
	doc := minimal + "listen: 0.0.0.0:9090\n"
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "admin.token_env") {
		t.Fatalf("err = %v", err)
	}
	doc += "admin:\n  token_env: AG_ORCH_ADMIN_TOKEN\n"
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "AG_ORCH_ADMIN_TOKEN is empty") {
		t.Fatalf("err = %v", err)
	}
	e := map[string]string{"AG_ORCH_TOKEN": "x", "AG_ORCH_ADMIN_TOKEN": "y"}
	if _, err := parse(t, doc, e); err != nil {
		t.Fatalf("with a token it must be accepted: %v", err)
	}
}

func TestAgentsCannotBeGrantedPowersThatBypassPolicy(t *testing.T) {
	for op := range ForbiddenAgentOps {
		doc := minimal + "    operations: [\"repo.read\", \"" + op + "\"]\n"
		_, err := parse(t, doc, goodEnv)
		if err == nil || !strings.Contains(err.Error(), op) {
			t.Errorf("operation %s must be refused, err = %v", op, err)
		}
	}
}

func TestServiceCredentialsCannotBePassedToExecutors(t *testing.T) {
	for _, name := range []string{"AG_ORCH_TOKEN", "AG_TOKEN"} {
		doc := minimal + "    pass_env: [PATH, " + name + "]\n"
		if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "service credential") {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	doc := minimal + "    env: {AG_TOKEN: stolen}\n"
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "reserved") {
		t.Errorf("AG_* in env must be refused: %v", err)
	}
}

func TestOneAgentPerExecutor(t *testing.T) {
	doc := minimal + `  - name: other
    type: command
    command: [x]
    agent: docs-bot
`
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "already used by executor") {
		t.Fatalf("err = %v", err)
	}
}

func TestRoutesAndDefaults(t *testing.T) {
	doc := minimal + `  - name: second
    type: command
    command: [x]
`
	doc = strings.Replace(doc, "  - name: acme/web", `  - name: acme/web
    schedule:
      default_executor: second
      routes:
        - {executor: docs-bot, title: "^docs:", scope: "docs/**"}`, 1)
	c, err := parse(t, doc, goodEnv)
	if err != nil {
		t.Fatal(err)
	}
	if got := c.Repos[0].Schedule; got.Default != "second" || len(got.Routes) != 1 {
		t.Fatalf("schedule = %+v", got)
	}
	if c.Executor("second") == nil || c.Executor("nope") != nil {
		t.Fatal("Executor lookup broken")
	}
	// With two executors and no default there is no implicit default.
	doc2 := minimal + "  - name: second\n    type: command\n    command: [x]\n"
	c2, err := parse(t, doc2, goodEnv)
	if err != nil || c2.Repos[0].Schedule.Default != "" {
		t.Fatalf("default = %q err=%v", c2.Repos[0].Schedule.Default, err)
	}
}

func TestDurationsBoundsAndGlobs(t *testing.T) {
	doc := minimal + "    timeout: 56m\n"
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "executors[0].timeout") {
		t.Fatalf("err = %v", err)
	}
	doc = strings.Replace(minimal, "  - name: acme/web", "  - name: acme/web\n    review: {protected: [\"a[\"]}", 1)
	if _, err := parse(t, doc, goodEnv); err == nil || !strings.Contains(err.Error(), "review") {
		t.Fatalf("bad glob accepted: %v", err)
	}
}

func TestRedactedOutputHasNoSecrets(t *testing.T) {
	c, err := parse(t, minimal, goodEnv)
	if err != nil {
		t.Fatal(err)
	}
	out := c.Redacted()
	if strings.Contains(out, "secretsecret") || !strings.Contains(out, "AG_ORCH_TOKEN") {
		t.Fatalf("redacted config:\n%s", out)
	}
}

// A token pasted where the NAME of a variable belongs must not come back in an error message, in the effective
// configuration, or in a later "unset" message.
func TestACredentialPastedAsAVariableNameIsNeverEchoed(t *testing.T) {
	const tok = "agt_cred_0123456789abcdef0123456789abcdef"
	for _, key := range []string{"token_env", "reviewer_token_env", "advisor_token_env"} {
		doc := strings.Replace(minimal, "url: http://127.0.0.1:8080", "url: http://127.0.0.1:8080\n  "+key+": "+tok, 1)
		_, err := parse(t, doc, goodEnv)
		if err == nil || !strings.Contains(err.Error(), "looks like one") {
			t.Fatalf("%s: err = %v", key, err)
		}
		if strings.Contains(err.Error(), tok) || strings.Contains(err.Error(), "agt_cred") {
			t.Fatalf("%s: the credential is echoed: %v", key, err)
		}
	}
	doc := minimal + "admin:\n  token_env: " + tok + "\n"
	if _, err := parse(t, doc, goodEnv); err == nil || strings.Contains(err.Error(), tok) {
		t.Fatalf("admin.token_env: %v", err)
	}
	long := strings.Repeat("x", 80)
	if _, err := parse(t, strings.Replace(minimal, "url: http://127.0.0.1:8080", "url: http://127.0.0.1:8080\n  token_env: "+long, 1), goodEnv); err == nil || strings.Contains(err.Error(), long) {
		t.Fatalf("an 80-character 'name' must be refused without being shown: %v", err)
	}
	// pass_env entries too
	doc = minimal + "    pass_env: [PATH, " + tok + "]\n"
	if _, err := parse(t, doc, goodEnv); err == nil || strings.Contains(err.Error(), tok) {
		t.Fatalf("pass_env: %v", err)
	}
}

func TestExecutorEnvironmentValuesAreRedactedInTheEffectiveConfiguration(t *testing.T) {
	doc := minimal + "    env: {API_KEY: sk-live-secret-value, MODE: batch}\n"
	c, err := parse(t, doc, goodEnv)
	if err != nil {
		t.Fatal(err)
	}
	out := c.Redacted()
	if strings.Contains(out, "sk-live-secret-value") || strings.Contains(out, "batch") {
		t.Fatalf("an environment value is shown:\n%s", out)
	}
	if !strings.Contains(out, "API_KEY: "+RedactedValue) || !strings.Contains(out, "MODE: "+RedactedValue) {
		t.Fatalf("the variable names are not kept:\n%s", out)
	}
	if c.Execs[0].Env["API_KEY"] != "sk-live-secret-value" {
		t.Fatal("redaction changed the live configuration")
	}
}

func TestEmptyAndInvalidFiles(t *testing.T) {
	if _, err := parse(t, "", goodEnv); err == nil {
		t.Fatal("an empty file has no repositories")
	}
	if _, err := parse(t, "repos: [", goodEnv); err == nil {
		t.Fatal("invalid YAML accepted")
	}
	if _, err := Load("/definitely/not/here.yaml", nil); err == nil {
		t.Fatal("missing file accepted")
	}
}

// The simulated "scripted" executor was removed from the product: neither its type nor its settings are accepted.
func TestTheSimulatedExecutorIsGone(t *testing.T) {
	if _, err := parse(t, minimal+"  - name: sim\n    type: scripted\n    scripted: {files: {a: b}}\n", goodEnv); err == nil {
		t.Fatal("a scripted executor was accepted")
	}
	if _, err := parse(t, minimal+"    scripted: {files: {a: b}}\n", goodEnv); err == nil || !strings.Contains(err.Error(), "scripted") {
		t.Fatalf("the scripted settings of a command executor were accepted: %v", err)
	}
}
