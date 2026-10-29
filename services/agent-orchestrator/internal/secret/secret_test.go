package secret

import (
	"bytes"
	"encoding/json"
	"fmt"
	"log/slog"
	"strings"
	"testing"
)

const tok = "agt_abcdef0123456789_SECRETSECRETSECRET"

func TestNoFormatterLeaksTheValue(t *testing.T) {
	s := New(tok)
	outputs := []string{
		fmt.Sprint(s), fmt.Sprintf("%v", s), fmt.Sprintf("%+v", s), fmt.Sprintf("%#v", s),
		fmt.Sprintf("%s", s), fmt.Sprintf("%q", s), fmt.Sprintf("%x", s), fmt.Sprint(&s),
		fmt.Sprint(struct{ T Secret }{s}), fmt.Sprintf("%+v", struct{ T Secret }{s}),
	}
	for _, o := range outputs {
		if strings.Contains(o, "SECRET") || !strings.Contains(o, Redacted) {
			t.Errorf("formatter leaked or lost the marker: %q", o)
		}
	}
	if s.Reveal() != tok {
		t.Fatal("Reveal must return the value")
	}
}

func TestJSONAndSlogNeverLeak(t *testing.T) {
	s := New(tok)
	b, err := json.Marshal(map[string]any{"token": s})
	if err != nil || strings.Contains(string(b), "SECRET") {
		t.Fatalf("json leaked: %s (%v)", b, err)
	}
	var buf bytes.Buffer
	slog.New(slog.NewJSONHandler(&buf, nil)).Info("x", "token", s)
	slog.New(slog.NewTextHandler(&buf, nil)).Info("x", slog.Any("token", s))
	if strings.Contains(buf.String(), "SECRET") {
		t.Fatalf("slog leaked: %s", buf.String())
	}
}

func TestUnmarshal(t *testing.T) {
	var resp struct {
		Token Secret `json:"token"`
	}
	if err := json.Unmarshal([]byte(`{"token":"`+tok+`"}`), &resp); err != nil {
		t.Fatal(err)
	}
	if resp.Token.Reveal() != tok || resp.Token.Empty() {
		t.Fatal("not decoded")
	}
	if err := json.Unmarshal([]byte(`{"token":null}`), &resp); err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal([]byte(`{"token":"a\"b"}`), &resp); err == nil {
		t.Fatal("escapes must be rejected")
	}
	if err := json.Unmarshal([]byte(`{"token":5}`), &resp); err == nil {
		t.Fatal("non-strings must be rejected")
	}
}

func TestScrub(t *testing.T) {
	s := New(tok)
	got := Scrub("curl -H 'Authorization: Bearer "+tok+"' and again "+tok, s)
	if strings.Contains(got, "SECRET") || strings.Count(got, Redacted) != 2 {
		t.Fatalf("Scrub = %q", got)
	}
	if got := Scrub("plain text", New("short")); got != "plain text" {
		t.Fatalf("short secrets must not be scrubbed: %q", got)
	}
	if got := Scrub("x", Secret{}); got != "x" {
		t.Fatal("empty secret must be ignored")
	}
}

func TestPendingSuffixLength(t *testing.T) {
	for _, tc := range []struct {
		text, value string
		want        int
	}{
		{"working\nparen", "parent-value", 5}, {"parent-value", "parent-value", 0},
		{"public", "parent-value", 0}, {"x", "", 0}, {"abcab", "abcabc", 5},
		{"progress λ", "λ-secret", len("λ")},
		{"", "secret", 0}, {"text", "x", 0},
		{strings.Repeat("a", 10000) + "c", strings.Repeat("a", 10000) + "b", 0},
	} {
		if got := PendingSuffixLength(tc.text, tc.value); got != tc.want {
			t.Errorf("pending suffix length = %d, want %d", got, tc.want)
		}
	}
}
