package config

import (
	"strings"
	"testing"
)

const modelDoc = minimal + `
llm:
  base_url: https://llm.example.org/api/v1
  model: vendor/some-model
  api_key_env: AG_LLM_KEY
`

func withModelMode(doc string) string {
	return strings.Replace(doc, "  - name: acme/web\n", "  - name: acme/web\n    review:\n      mode: model\n", 1)
}

func llmEnv() map[string]string {
	return map[string]string{"AG_ORCH_TOKEN": goodEnv["AG_ORCH_TOKEN"], "AG_LLM_KEY": "sk-not-a-real-key-0123456789"}
}

func TestHeuristicIsTheDefaultAndNeedsNoModelSettings(t *testing.T) {
	c, err := parse(t, minimal, goodEnv)
	if err != nil {
		t.Fatal(err)
	}
	r := c.Repos[0]
	if r.Advisor.Mode != ModeHeuristic || r.Review.Mode != ModeHeuristic || r.Index.Mode != ModeHeuristic || c.UsesModel() {
		t.Fatalf("modes = %q %q %q", r.Advisor.Mode, r.Review.Mode, r.Index.Mode)
	}
}

func TestModelModeNeedsAnEndpointAModelAndTheKeyVariable(t *testing.T) {
	c, err := parse(t, withModelMode(modelDoc), llmEnv())
	if err != nil {
		t.Fatal(err)
	}
	if !c.UsesModel() || c.LLM.Model != "vendor/some-model" || c.LLM.RetryCount() != 3 || c.LLM.RequestsPerHourLimit() != 200 {
		t.Fatalf("llm = %+v", c.LLM)
	}
	for name, doc := range map[string]string{
		"no endpoint": strings.Replace(modelDoc, "  base_url: https://llm.example.org/api/v1\n", "", 1),
		"no model":    strings.Replace(modelDoc, "  model: vendor/some-model\n", "", 1),
	} {
		if _, err := parse(t, withModelMode(doc), llmEnv()); err == nil || !strings.Contains(err.Error(), "llm.") {
			t.Errorf("%s: err = %v", name, err)
		}
	}
	// the key variable must be set and non-empty
	if _, err := parse(t, withModelMode(modelDoc), goodEnv); err == nil || !strings.Contains(err.Error(), "AG_LLM_KEY") {
		t.Errorf("an unset key variable: err = %v", err)
	}
	// without api_key_env nothing is sent as a credential (a gateway adds it)
	noKey := strings.Replace(modelDoc, "  api_key_env: AG_LLM_KEY\n", "", 1)
	if _, err := parse(t, withModelMode(noKey), goodEnv); err != nil {
		t.Errorf("a keyless gateway configuration was refused: %v", err)
	}
}

func TestTheModelSettingsAreNotRequiredWhileEveryComponentIsHeuristic(t *testing.T) {
	if _, err := parse(t, minimal+"llm:\n  timeout: 30s\n", goodEnv); err != nil {
		t.Fatal(err)
	}
}

func TestAKeyCannotBePastedIntoTheConfiguration(t *testing.T) {
	const pasted = "sk-or-v1-0123456789abcdef0123456789abcdef"
	doc := strings.Replace(modelDoc, "AG_LLM_KEY", pasted, 1)
	_, err := parse(t, withModelMode(doc), llmEnv())
	if err == nil || !strings.Contains(err.Error(), "llm.api_key_env") {
		t.Fatalf("err = %v", err)
	}
	if strings.Contains(err.Error(), pasted) {
		t.Fatalf("the error echoes the key: %v", err)
	}
	// nor in the URL
	for _, u := range []string{"https://user:pw@llm.example.org/v1", "https://llm.example.org/v1?key=abc"} {
		doc := strings.Replace(modelDoc, "https://llm.example.org/api/v1", u, 1)
		if _, err := parse(t, withModelMode(doc), llmEnv()); err == nil || !strings.Contains(err.Error(), "llm.base_url") {
			t.Errorf("%s: err = %v", u, err)
		}
	}
}

func TestPlainHTTPToARemoteModelEndpointIsRefusedButLoopbackIsFine(t *testing.T) {
	remote := strings.Replace(modelDoc, "https://llm.example.org/api/v1", "http://llm.example.org/v1", 1)
	if _, err := parse(t, withModelMode(remote), llmEnv()); err == nil || !strings.Contains(err.Error(), "llm.base_url") {
		t.Fatalf("err = %v", err)
	}
	local := strings.Replace(modelDoc, "https://llm.example.org/api/v1", "http://127.0.0.1:11434/v1", 1)
	if _, err := parse(t, withModelMode(local), llmEnv()); err != nil {
		t.Fatal(err)
	}
}

func TestAnUnknownModeAndBadBoundsAreReported(t *testing.T) {
	bad := strings.Replace(modelDoc, "  - name: acme/web\n", "  - name: acme/web\n    index:\n      mode: telepathy\n", 1)
	if _, err := parse(t, bad, llmEnv()); err == nil || !strings.Contains(err.Error(), "index.mode") {
		t.Fatalf("err = %v", err)
	}
	bounds := modelDoc + "  budget:\n    requests_per_hour: -1\n  retries: 20\n"
	_, err := parse(t, withModelMode(bounds), llmEnv())
	if err == nil || !strings.Contains(err.Error(), "llm.budget") || !strings.Contains(err.Error(), "llm.retries") {
		t.Fatalf("err = %v", err)
	}
}

func TestTheAuthorizationHeaderCannotBeSetFromTheFile(t *testing.T) {
	doc := modelDoc + "  headers:\n    Authorization: Bearer x\n"
	if _, err := parse(t, withModelMode(doc), llmEnv()); err == nil || !strings.Contains(err.Error(), "llm.headers") {
		t.Fatalf("err = %v", err)
	}
}
