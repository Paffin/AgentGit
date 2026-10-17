package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/app"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/config"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/testutil/fakeagwp"
)

func writeConfig(t *testing.T, url string) string {
	t.Helper()
	doc := fmt.Sprintf(`
server:
  url: %s
state_dir: %s
listen: 127.0.0.1:0
repos:
  - name: acme/web
executors:
  - name: docs-bot
    type: scripted
    scripted:
      files:
        docs/a.md: "hi\n"
`, url, t.TempDir())
	p := filepath.Join(t.TempDir(), "orchestrator.yaml")
	if err := os.WriteFile(p, []byte(doc), 0o600); err != nil {
		t.Fatal(err)
	}
	return p
}

func env(m map[string]string) func(string) string { return func(k string) string { return m[k] } }

func exec(t *testing.T, getenv func(string) string, args ...string) (int, string, string) {
	t.Helper()
	var out, errb bytes.Buffer
	code := run(args, getenv, &out, &errb)
	return code, out.String(), errb.String()
}

func TestVersionAndHelp(t *testing.T) {
	code, out, _ := exec(t, env(nil), "version")
	if code != 0 || !strings.Contains(out, "agent-orchestrator") {
		t.Fatalf("version: %d %q", code, out)
	}
	code, out, _ = exec(t, env(nil), "help")
	if code != 0 || !strings.Contains(out, "usage: orchestrator") {
		t.Fatalf("help: %d %q", code, out)
	}
}

func TestACommandWithoutAConfigurationFileExplainsWhatToDo(t *testing.T) {
	code, _, errs := exec(t, env(nil), "check")
	if code != 2 || !strings.Contains(errs, "-config") || !strings.Contains(errs, "AG_ORCH_CONFIG") {
		t.Fatalf("%d %q", code, errs)
	}
}

func TestConfigPathComesFromTheEnvironmentToo(t *testing.T) {
	p := writeConfig(t, "http://127.0.0.1:1")
	code, out, errs := exec(t, env(map[string]string{"AG_ORCH_CONFIG": p}), "config")
	if code != 0 || !strings.Contains(out, "acme/web") {
		t.Fatalf("%d %q %q", code, out, errs)
	}
}

func TestUnknownCommandsAndFlagsAreUsageErrors(t *testing.T) {
	p := writeConfig(t, "http://127.0.0.1:1")
	if code, _, errs := exec(t, env(map[string]string{"AG_ORCH_TOKEN": "x"}), "frobnicate", "-config", p); code != 2 || !strings.Contains(errs, "unknown command") {
		t.Fatalf("%d %q", code, errs)
	}
	if code, _, _ := exec(t, env(nil), "run", "-nonsense"); code != 2 {
		t.Fatalf("an unknown flag must exit 2, got %d", code)
	}
}

func TestConfigAndOpsDoNotNeedACredential(t *testing.T) {
	p := writeConfig(t, "http://127.0.0.1:1")
	code, out, errs := exec(t, env(nil), "config", "-config", p)
	if code != 0 {
		t.Fatalf("config without a token: %d %s", code, errs)
	}
	if !strings.Contains(out, "acme/web") {
		t.Fatalf("effective configuration missing the repository:\n%s", out)
	}
	code, out, errs = exec(t, env(nil), "ops", "-config", p)
	if code != 0 {
		t.Fatalf("ops without a token: %d %s", code, errs)
	}
	var doc struct {
		Orchestrator []string `json:"orchestrator"`
		Reviewer     []string `json:"reviewer"`
		Advisor      []string `json:"advisor"`
		AgentDefault []string `json:"agent"`
	}
	if err := json.Unmarshal([]byte(out), &doc); err != nil {
		t.Fatalf("%v\n%s", err, out)
	}
	if len(doc.Orchestrator) == 0 || len(doc.Reviewer) == 0 || len(doc.Advisor) == 0 || len(doc.AgentDefault) == 0 {
		t.Fatalf("ops document incomplete: %+v", doc)
	}
	for _, o := range append(append([]string{}, doc.Orchestrator...), doc.AgentDefault...) {
		if config.ForbiddenAgentOps[o] && o != "credentials.mint" && o != "credentials.revoke" && o != "agent.register" && o != "integrate.validate" {
			t.Errorf("ops lists %q", o)
		}
	}
}

// The example in examples/ is what the operator guide shows: it must stay valid.
func TestTheShippedExampleConfigurationIsValid(t *testing.T) {
	const path = "../../examples/orchestrator.yaml"
	cfg, err := config.Load(path, env(map[string]string{"AG_ORCH_TOKEN": "x"}))
	if err != nil {
		t.Fatal(err)
	}
	if len(cfg.Repos) != 1 || cfg.Repos[0].Name != "acme/web" || len(cfg.Execs) != 2 {
		t.Fatalf("example = %+v", cfg)
	}
	if code, out, errs := exec(t, env(nil), "config", "-config", path); code != 0 || !strings.Contains(out, "docs-bot") || !strings.Contains(out, "shell-bot") {
		t.Fatalf("config: %d %s %s", code, out, errs)
	}
	for _, id := range []string{"orchestrator", "reviewer", "advisor", "agent"} {
		if code, out, errs := exec(t, env(nil), "ops", "-config", path, "-for", id, "-flags"); code != 0 || !strings.Contains(out, "--operation repo.read") {
			t.Fatalf("ops %s: %d %q %s", id, code, out, errs)
		}
	}
	for _, f := range []string{"../../examples/shell-bot.sh", "../../examples/worked-example.sh"} {
		if st, err := os.Stat(f); err != nil || st.Size() == 0 {
			t.Errorf("%s: %v", f, err)
		}
	}
}

func TestOpsCanBePrintedAsMintFlags(t *testing.T) {
	p := writeConfig(t, "http://127.0.0.1:1")
	code, out, errs := exec(t, env(nil), "ops", "-config", p, "-for", "reviewer", "-flags")
	if code != 0 {
		t.Fatalf("%d %s", code, errs)
	}
	want := "--operation repo.read --operation stream.read --operation proposal.read --operation proposal.review --operation tree.read --operation history.read\n"
	if out != want {
		t.Fatalf("flags = %q, want %q", out, want)
	}
	code, out, _ = exec(t, env(nil), "ops", "-config", p, "-for", "advisor")
	if code != 0 || !strings.Contains(out, `"advisor"`) || strings.Contains(out, `"orchestrator"`) {
		t.Fatalf("-for must narrow the document: %d %s", code, out)
	}
	if code, _, errs := exec(t, env(nil), "ops", "-config", p, "-flags"); code != 2 || !strings.Contains(errs, "-for") {
		t.Fatalf("-flags without -for: %d %q", code, errs)
	}
	if code, _, errs := exec(t, env(nil), "ops", "-config", p, "-for", "nobody"); code != 2 || !strings.Contains(errs, "unknown identity") {
		t.Fatalf("unknown identity: %d %q", code, errs)
	}
}

func TestRunAndCheckStillRequireTheCredential(t *testing.T) {
	p := writeConfig(t, "http://127.0.0.1:1")
	code, _, errs := exec(t, env(nil), "check", "-config", p)
	if code != 2 || !strings.Contains(errs, "AG_ORCH_TOKEN") {
		t.Fatalf("%d %q", code, errs)
	}
}

func TestCheckExitCodeFollowsTheReport(t *testing.T) {
	fake := fakeagwp.New()
	t.Cleanup(fake.Close)
	fake.AddRepo("acme/web")
	fake.SetHead("acme/web", "main", fake.Commit("acme/web", nil, map[string]string{"README.md": "x\n"}, "user:root", "seed"))
	p := writeConfig(t, fake.URL)
	cfg, err := config.Load(p, env(map[string]string{"AG_ORCH_TOKEN": "x"}))
	if err != nil {
		t.Fatal(err)
	}
	good := env(map[string]string{
		"AG_ORCH_TOKEN":     fake.AddToken(fakeagwp.TokenSpec{Identity: "service:orchestrator", Role: "admin", Ops: app.OrchestratorOps(cfg)}),
		"AG_REVIEWER_TOKEN": fake.AddToken(fakeagwp.TokenSpec{Identity: "service:reviewer", Role: "reviewer", Ops: app.ReviewerOps()}),
		"AG_ADVISOR_TOKEN":  fake.AddToken(fakeagwp.TokenSpec{Identity: "service:advisor", Role: "reviewer", Ops: app.AdvisorOps()}),
	})
	code, out, errs := exec(t, good, "check", "-config", p)
	if code != 0 {
		t.Fatalf("check: %d\n%s\n%s", code, out, errs)
	}
	code, out, _ = exec(t, good, "check", "-json", "-config", p)
	var rep struct {
		OK    bool             `json:"ok"`
		Items []map[string]any `json:"items"`
	}
	if err := json.Unmarshal([]byte(out), &rep); err != nil || code != 0 || !rep.OK || len(rep.Items) == 0 {
		t.Fatalf("check -json: %d %v\n%s", code, err, out)
	}
	for _, tok := range []string{good("AG_ORCH_TOKEN"), good("AG_REVIEWER_TOKEN")} {
		if strings.Contains(out, tok) {
			t.Fatal("the check output contains a credential")
		}
	}
	bad := env(map[string]string{"AG_ORCH_TOKEN": "agt_wrong_wrong_wrong"})
	if code, out, _ := exec(t, bad, "check", "-config", p); code != 1 || !strings.Contains(out, "ERROR") {
		t.Fatalf("a failing check must exit 1: %d\n%s", code, out)
	}
}

func TestTheFileCannotCarryAToken(t *testing.T) {
	p := filepath.Join(t.TempDir(), "c.yaml")
	if err := os.WriteFile(p, []byte("server:\n  url: http://127.0.0.1:1\n  token: agt_secret\nstate_dir: /tmp/x\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	code, _, errs := exec(t, env(map[string]string{"AG_ORCH_TOKEN": "x"}), "config", "-config", p)
	if code != 2 || !strings.Contains(errs, "never configured in the file") {
		t.Fatalf("%d %q", code, errs)
	}
	if strings.Contains(errs, "agt_secret") {
		t.Fatal("the error echoes the token")
	}
}
