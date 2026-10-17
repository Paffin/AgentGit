//go:build linux

package integration

import (
	"context"
	"encoding/json"
	"net/url"
	"regexp"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/semindex"
)

const configGo = `package config

// ParseConfig reads the configuration.
func ParseConfig(text string) (Config, error) {
	var c Config
	c.Name = text
	c.Level = 1
	return c, nil
}

type Config struct {
	Name  string
	Level int
}
`

const mainGo = `package main

import "example.com/web/internal/config"

func main() {
	c, _ := config.ParseConfig("web")
	_ = c
}
`

type symbolsReply struct {
	Head    string            `json:"head"`
	Symbols []semindex.Symbol `json:"symbols"`
}

func (o *orchestrator) symbols(q string) symbolsReply {
	o.t.Helper()
	code, body := o.get("/index/symbols?mode=exact&q=" + url.QueryEscape(q))
	if code != 200 {
		o.t.Fatalf("/index/symbols = %d %s", code, body)
	}
	var r symbolsReply
	if err := json.Unmarshal([]byte(body), &r); err != nil {
		o.t.Fatal(err)
	}
	return r
}

// The semantic index follows the stream: a symbol added by a landed change
// becomes findable (with file and line), a renamed one disappears from its old
// name, updates are incremental (only the changed files are fetched), and
// open proposals that change the same definition are reported as overlapping.
func TestSemanticIndexFollowsIntegrationsAndSeesOverlappingProposals(t *testing.T) {
	w := newWorld(t, worldOptions{})
	w.seed(map[string]string{"internal/config/config.go": configGo, "cmd/web/main.go": mainGo, "README.md": "# web\n"})

	o := w.startOrchestrator(orchConfig{
		Repo: "    schedule: {enabled: false}\n    advisor: {enabled: false}\n    review: {enabled: false}\n",
	})
	o.waitReady()

	// The initial index covers the seeded stream.
	eventually(t, "the initial index", 30*time.Second, func() bool { return len(o.symbols("ParseConfig").Symbols) == 1 })
	r := o.symbols("ParseConfig")
	if head, err := w.alice.ResolveRev(context.Background(), w.repo, "main"); err != nil || r.Head != head {
		t.Fatalf("the index is at %s, the stream at %s (%v)", r.Head, head, err)
	}
	def := r.Symbols[0].Definition
	if len(def) != 1 || def[0].Path != "internal/config/config.go" || def[0].Kind != "func" || def[0].Line != 4 || !def[0].Exported {
		t.Fatalf("ParseConfig = %+v", r.Symbols[0])
	}
	if len(o.symbols("NormalizeEmail").Symbols) != 0 {
		t.Fatal("a symbol that does not exist yet was found")
	}
	code, body := o.get("/index/files?symbol=ParseConfig")
	if code != 200 {
		t.Fatalf("/index/files = %d %s", code, body)
	}
	var usage struct {
		Usage semindex.Usage `json:"usage"`
	}
	if err := json.Unmarshal([]byte(body), &usage); err != nil {
		t.Fatal(err)
	}
	if len(usage.Usage.DefinedIn) != 1 || len(usage.Usage.ReferencedIn) == 0 || usage.Usage.ReferencedIn[0].Path != "cmd/web/main.go" {
		t.Fatalf("usage = %+v", usage.Usage)
	}

	// An integration event: a person lands a change that adds a symbol.
	add, _ := w.propose("ws-mail", "Add email normalisation",
		put("internal/mail/mail.go", "package mail\n\nimport \"strings\"\n\n// NormalizeEmail lower-cases an address.\nfunc NormalizeEmail(addr string) string {\n\treturn strings.ToLower(strings.TrimSpace(addr))\n}\n"))
	w.land(add)
	eventually(t, "the new symbol to be indexed", 30*time.Second, func() bool { return len(o.symbols("NormalizeEmail").Symbols) == 1 })
	nr := o.symbols("NormalizeEmail")
	nd := nr.Symbols[0].Definition
	if len(nd) != 1 || nd[0].Path != "internal/mail/mail.go" || nd[0].Line != 6 || nd[0].Kind != "func" {
		t.Fatalf("NormalizeEmail = %+v", nr.Symbols[0])
	}
	if len(o.symbols("ParseConfig").Symbols) != 1 {
		t.Fatal("an untouched symbol was lost")
	}

	// A rename: the old definition goes, the new one appears; the caller still mentions the old name.
	rename, _ := w.propose("ws-rename", "Rename ParseConfig to LoadConfig",
		put("internal/config/config.go", strings.ReplaceAll(configGo, "ParseConfig", "LoadConfig")))
	w.land(rename)
	eventually(t, "the rename to be indexed", 30*time.Second, func() bool { return len(o.symbols("LoadConfig").Symbols) == 1 })
	if len(o.symbols("ParseConfig").Symbols) != 0 {
		t.Fatalf("the old name is still defined: %+v", o.symbols("ParseConfig"))
	}

	// The updates were incremental: one changed file each, not a rebuild.
	updates := regexp.MustCompile(`index updated" .*files=(\d+) symbols=(\d+) changed=(\d+) removed=(\d+)`).FindAllStringSubmatch(o.logs.String(), -1)
	if len(updates) < 3 {
		t.Fatalf("expected the initial build and two updates in the log, got %d:\n%s", len(updates), o.logs.String())
	}
	if first := updates[0]; first[3] != "2" {
		t.Errorf("the initial build should index the two source files (README.md is not source): %v", first)
	}
	for i, u := range updates[len(updates)-2:] {
		if u[3] != "1" || u[4] != "0" {
			t.Errorf("update %d re-read %s files and removed %s; want an incremental 1/0: %v", i, u[3], u[4], u)
		}
	}

	// Two open proposals that change the body of the same function are reported as overlapping,
	// although their line ranges do not touch.
	variant := func(edit func(string) string) string {
		return edit(strings.ReplaceAll(configGo, "ParseConfig", "LoadConfig"))
	}
	one, _ := w.propose("ws-name", "Trim the name",
		put("internal/config/config.go", variant(func(s string) string {
			return strings.Replace(s, "c.Name = text", "c.Name = strings.TrimSpace(text)", 1)
		})))
	two, _ := w.propose("ws-level", "Raise the default level",
		put("internal/config/config.go", variant(func(s string) string { return strings.Replace(s, "c.Level = 1", "c.Level = 3", 1) })))
	var ov []semindex.Overlap
	eventually(t, "the overlap to be reported", 30*time.Second, func() bool {
		code, body := o.get("/index/overlaps")
		if code != 200 {
			return false
		}
		var reply struct {
			Overlaps []semindex.Overlap `json:"overlaps"`
		}
		if json.Unmarshal([]byte(body), &reply) != nil {
			return false
		}
		ov = reply.Overlaps
		return len(ov) > 0
	})
	if o1 := ov[0]; o1.Kind != semindex.KindDefinition || !((o1.A == one.ID && o1.B == two.ID) || (o1.A == two.ID && o1.B == one.ID)) ||
		len(o1.Symbols) != 1 || o1.Symbols[0].Name != "LoadConfig" {
		t.Fatalf("overlaps = %+v", ov)
	}
	code, cbody := o.get("/index/changes?proposal=" + one.ID)
	if code != 200 || !strings.Contains(cbody, "LoadConfig") {
		t.Fatalf("/index/changes = %d %s", code, cbody)
	}
}
