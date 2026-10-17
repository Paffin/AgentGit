package advisor

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"reflect"
	"strings"
	"testing"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/textdiff"
)

func f(s string) *File { return &File{Content: []byte(s)} }

func suggest(t *testing.T, in Input) Suggestion {
	t.Helper()
	s, err := Rules{}.Suggest(context.Background(), in)
	if err != nil {
		t.Fatal(err)
	}
	return s
}

func TestSuggestTable(t *testing.T) {
	tests := []struct {
		name       string
		in         Input
		class      string
		complete   bool
		confidence string
		resolved   string
		regions    int
	}{
		{
			name:  "adjacent edits are applied together",
			in:    Input{Path: "lib.go", Base: f("a\nb\nc\nd\n"), Ours: f("a\nB\nc\nd\n"), Theirs: f("a\nb\nC\nd\n")},
			class: ClassAdjacent, complete: true, confidence: Medium, resolved: "a\nB\nC\nd\n", regions: 1,
		},
		{
			name:  "adjacent insertion next to a replacement",
			in:    Input{Path: "lib.go", Base: f("a\nb\nc\n"), Ours: f("a\nB\nc\n"), Theirs: f("a\nb\nNEW\nc\n")},
			class: ClassAdjacent, complete: true, confidence: Medium, resolved: "a\nB\nNEW\nc\n", regions: 1,
		},
		{
			name:  "reformatting versus a real change keeps the change",
			in:    Input{Path: "x.py", Base: f("def f( a ):\n    return a\n"), Ours: f("def f(a):\n    return a\n"), Theirs: f("def f( a, b ):\n    return a\n")},
			class: ClassWhitespace, complete: true, confidence: High, resolved: "def f( a, b ):\n    return a\n", regions: 1,
		},
		{
			name:  "the other way round",
			in:    Input{Path: "x.py", Base: f("x = 1\n"), Ours: f("x = 2\n"), Theirs: f("x  =  1\n")},
			class: ClassWhitespace, complete: true, confidence: High, resolved: "x = 2\n", regions: 1,
		},
		{
			name:  "same change, different spacing: stream formatting wins",
			in:    Input{Path: "x.py", Base: f("x = 1\n"), Ours: f("x = 2\n"), Theirs: f("x   =   2\n")},
			class: ClassWhitespace, complete: true, confidence: High, resolved: "x = 2\n", regions: 1,
		},
		{
			name:  "line endings only",
			in:    Input{Path: "a.txt", Base: f("a\nb\n"), Ours: f("a\r\nb\r\n"), Theirs: f("a\nB\n")},
			class: ClassWhitespace, complete: true, confidence: High, resolved: "a\nB\n", regions: 1,
		},
		{
			name: "go imports added on both sides are united and kept sorted",
			in: Input{Path: "cmd/main.go",
				Base:   f("package main\n\nimport (\n\t\"fmt\"\n)\n"),
				Ours:   f("package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n"),
				Theirs: f("package main\n\nimport (\n\t\"fmt\"\n\t\"strings\"\n)\n")},
			class: ClassUnion, complete: true, confidence: High,
			resolved: "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n\t\"strings\"\n)\n", regions: 1,
		},
		{
			name: "python imports",
			in: Input{Path: "app.py",
				Base:   f("import os\n\nrun()\n"),
				Ours:   f("import os\nimport sys\n\nrun()\n"),
				Theirs: f("import os\nimport json\n\nrun()\n")},
			class: ClassUnion, complete: true, confidence: High, resolved: "import os\nimport json\nimport sys\n\nrun()\n", regions: 1,
		},
		{
			name: "dependency lists in requirements.txt",
			in: Input{Path: "requirements.txt",
				Base: f("flask==2.0\n"), Ours: f("flask==2.0\nrequests>=2.28\n"), Theirs: f("flask==2.0\nclick==8.1\n")},
			class: ClassUnion, complete: true, confidence: High, resolved: "flask==2.0\nclick==8.1\nrequests>=2.28\n", regions: 1,
		},
		{
			name: "rust use declarations",
			in: Input{Path: "src/lib.rs",
				Base: f("use std::fmt;\n\nfn a() {}\n"), Ours: f("use std::fmt;\nuse std::io;\n\nfn a() {}\n"), Theirs: f("use std::fmt;\nuse std::path::Path;\n\nfn a() {}\n")},
			class: ClassUnion, complete: true, confidence: High, resolved: "use std::fmt;\nuse std::io;\nuse std::path::Path;\n\nfn a() {}\n", regions: 1,
		},
		{
			name: "entries that are not list entries are not united",
			in: Input{Path: "main.go",
				Base: f("package main\n\nfunc a() {}\n"), Ours: f("package main\n\nfunc a() {}\nfunc b() {}\n"), Theirs: f("package main\n\nfunc a() {}\nfunc c() {}\n")},
			class: ClassNeedsPerson, complete: false, regions: 1,
		},
		{
			name: "both sides added the same line at the same spot",
			in: Input{Path: "notes.txt",
				Base: f("start\nend\n"), Ours: f("start\nshared line of text\nmine\nend\n"), Theirs: f("start\nshared line of text\ntheirs\nend\n")},
			class: ClassSameLines, complete: true, confidence: Low, resolved: "start\nshared line of text\nmine\ntheirs\nend\n", regions: 1,
		},
		{
			name:  "braces alone are not 'the same lines'",
			in:    Input{Path: "a.go", Base: f("x\ny\n"), Ours: f("x\n}\nmine\ny\n"), Theirs: f("x\n}\ntheirs\ny\n")},
			class: ClassNeedsPerson, complete: false, regions: 1,
		},
		{
			name:  "the same line changed differently needs a person",
			in:    Input{Path: "a.go", Base: f("a\nb\nc\n"), Ours: f("a\nOURS\nc\n"), Theirs: f("a\nTHEIRS\nc\n")},
			class: ClassNeedsPerson, complete: false, regions: 1,
		},
		{
			name: "a file with one easy and one hard region is not completed",
			in: Input{Path: "a.go",
				Base:   f("1\n2\n3\n4\n5\n6\n7\n"),
				Ours:   f("1\nO2\n3\n4\n5\nO6\n7\n"),
				Theirs: f("1\n2\nT3\n4\n5\nT6\n7\n")},
			class: ClassMixed, complete: false, regions: 2,
		},
		{
			name: "two easy regions of different kinds",
			in: Input{Path: "a.go",
				Base:   f("1\n2\n3\n4\n5\n6\n7\n"),
				Ours:   f("1\nO2\n3\n4\n5\n6\n7\n"),
				Theirs: f("1\n2\nT3\n4\n5\n  6\n7\n")},
			class: ClassAdjacent, complete: true, confidence: Medium, resolved: "1\nO2\nT3\n4\n5\n  6\n7\n", regions: 1,
		},
		{
			name:  "deleted on one side, edited on the other",
			in:    Input{Path: "gone.txt", Base: f("a\n"), Ours: nil, Theirs: f("b\n")},
			class: ClassDeleteEdit, complete: false,
		},
		{
			name:  "deleted by the proposal",
			in:    Input{Path: "gone.txt", Base: f("a\n"), Ours: f("b\n"), Theirs: nil},
			class: ClassDeleteEdit, complete: false,
		},
		{
			name:  "binary",
			in:    Input{Path: "img.png", Base: f("a\x00b"), Ours: f("a\x00c"), Theirs: f("a\x00d")},
			class: ClassBinary, complete: false,
		},
		{
			name:  "invalid UTF-8",
			in:    Input{Path: "x.txt", Base: f("a"), Ours: f("\xff\xfe"), Theirs: f("b")},
			class: ClassBinary, complete: false,
		},
		{
			name:  "both added the path with the same content up to whitespace",
			in:    Input{Path: "new.txt", Ours: f("hello world\n"), Theirs: f("hello   world\n\n")},
			class: ClassWhitespace, complete: true, confidence: High, resolved: "hello world\n", regions: 1,
		},
		{
			name:  "both created requirements.txt",
			in:    Input{Path: "requirements.txt", Ours: f("# deps\nflask==2.0\n"), Theirs: f("click==8.1\nflask==2.0\n")},
			class: ClassUnion, complete: true, confidence: High, resolved: "# deps\nflask==2.0\nclick==8.1\n", regions: 1,
		},
		{
			name:  "both created a source file differently",
			in:    Input{Path: "new.go", Ours: f("package a\n"), Theirs: f("package b\n")},
			class: ClassBothAdded, complete: false, regions: 1,
		},
		{
			name:  "both sides removed it",
			in:    Input{Path: "gone.txt", Base: f("a\n")},
			class: ClassNeedsPerson, complete: false,
		},
		{
			name:  "the versions already merge cleanly (stale report)",
			in:    Input{Path: "a.txt", Base: f("1\n2\n3\n4\n5\n"), Ours: f("1\nO\n3\n4\n5\n"), Theirs: f("1\n2\n3\n4\nT\n")},
			class: ClassMixed, complete: true, confidence: High, resolved: "1\nO\n3\n4\nT\n",
		},
	}
	for _, tt := range tests {
		s := suggest(t, tt.in)
		if s.Class != tt.class || s.Complete != tt.complete || s.Confidence != tt.confidence || len(s.Regions) != tt.regions {
			t.Errorf("%s:\n got class=%s complete=%v confidence=%q regions=%d notes=%v\nwant class=%s complete=%v confidence=%q regions=%d",
				tt.name, s.Class, s.Complete, s.Confidence, len(s.Regions), s.Notes, tt.class, tt.complete, tt.confidence, tt.regions)
			continue
		}
		if tt.complete && string(s.Resolved) != tt.resolved {
			t.Errorf("%s: resolved\n%q\nwant\n%q", tt.name, s.Resolved, tt.resolved)
		}
		if !tt.complete && len(s.Resolved) != 0 {
			t.Errorf("%s: an incomplete suggestion must not carry a file", tt.name)
		}
		if s.Complete {
			sum := sha256.Sum256(s.Resolved)
			if s.SHA256 != hex.EncodeToString(sum[:]) {
				t.Errorf("%s: digest does not match the resolved file", tt.name)
			}
		}
		for _, r := range s.Regions {
			if r.Why == "" {
				t.Errorf("%s: region without an explanation: %+v", tt.name, r)
			}
		}
	}
}

func TestSuggestionsAreDeterministic(t *testing.T) {
	in := Input{Path: "cmd/main.go",
		Base:   f("package main\n\nimport (\n\t\"fmt\"\n)\n\nfunc a() {}\nfunc b() {}\n"),
		Ours:   f("package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\nfunc a() { println() }\nfunc b() {}\n"),
		Theirs: f("package main\n\nimport (\n\t\"fmt\"\n\t\"io\"\n)\n\nfunc a() {}\nfunc b() { x() }\n")}
	first := suggest(t, in)
	for i := 0; i < 25; i++ {
		if got := suggest(t, in); !reflect.DeepEqual(got, first) {
			t.Fatalf("run %d differs:\n%+v\n%+v", i, got, first)
		}
	}
}

func TestSizeLimitSkipsAnalysis(t *testing.T) {
	big := strings.Repeat("line\n", 100)
	s, _ := Rules{MaxFileBytes: 100}.Suggest(context.Background(), Input{Path: "a.txt", Base: f(big), Ours: f(big + "x\n"), Theirs: f(big + "y\n")})
	if s.Class != ClassBinary || len(s.Notes) == 0 {
		t.Fatalf("s = %+v", s)
	}
}

// The suggested file must contain every line that exactly one side added, so
// that no side's work disappears silently from a "complete" suggestion.
func TestCompleteSuggestionsKeepBothSidesWork(t *testing.T) {
	cases := []Input{
		{Path: "a.go", Base: f("a\nb\nc\nd\n"), Ours: f("a\nB\nc\nd\n"), Theirs: f("a\nb\nC\nd\n")},
		{Path: "m.go", Base: f("import \"a\"\n"), Ours: f("import \"a\"\nimport \"b\"\n"), Theirs: f("import \"a\"\nimport \"c\"\n")},
		{Path: "requirements.txt", Base: f("a==1\n"), Ours: f("a==1\nb==2\n"), Theirs: f("a==1\nc==3\n")},
	}
	for _, in := range cases {
		s := suggest(t, in)
		if !s.Complete {
			t.Fatalf("%s should be complete: %+v", in.Path, s)
		}
		for _, side := range []*File{in.Ours, in.Theirs} {
			for _, l := range strings.Split(string(side.Content), "\n") {
				if l != "" && !bytes.Contains(s.Resolved, []byte(l)) && !bytes.Contains(in.Base.Content, []byte(l)) {
					t.Errorf("%s: line %q of one side is missing from %q", in.Path, l, s.Resolved)
				}
			}
		}
	}
}

func TestHunksOverlap(t *testing.T) {
	h := func(a, e int) textdiff.Hunk { return textdiff.Hunk{AStart: a, AEnd: e} }
	cases := []struct {
		a, b textdiff.Hunk
		want bool
	}{
		{h(1, 3), h(3, 5), false}, // touching
		{h(1, 3), h(2, 4), true},
		{h(2, 2), h(2, 2), true},  // two insertions at one spot
		{h(2, 2), h(3, 3), false}, // different spots
		{h(2, 2), h(1, 3), true},  // insertion inside a replacement
		{h(3, 3), h(1, 3), false}, // insertion at the edge
		{h(1, 3), h(1, 3), true},
	}
	for _, c := range cases {
		if got := hunksOverlap(c.a, c.b); got != c.want {
			t.Errorf("hunksOverlap(%v,%v) = %v", c.a, c.b, got)
		}
		if got := hunksOverlap(c.b, c.a); got != c.want {
			t.Errorf("hunksOverlap(%v,%v) = %v (symmetry)", c.b, c.a, got)
		}
	}
}

func TestListLineDetection(t *testing.T) {
	yes := map[string][]string{
		"a.go":             {`"fmt"`, `import "os"`, `log "github.com/x/log"`, `_ "embed"`},
		"a.py":             {"import os", "from a.b import c, d", "import numpy as np"},
		"a.ts":             {`import { a } from "./a";`, `const x = require('x');`, `import "./side-effect"`},
		"a.rs":             {"use std::io;", "mod parser;", "pub use crate::a::{B, C};"},
		"a.c":              {`#include <stdio.h>`, `#include "x.h"`},
		"requirements.txt": {"flask==2.0", "requests", "numpy>=1.2,<2", "foo[bar]==1"},
		"go.mod":           {"github.com/a/b v1.2.3", "require golang.org/x/text v0.3.0"},
		"Cargo.toml":       {`serde = "1.0"`, `tokio = { version = "1", features = ["full"] }`},
		".gitignore":       {"*.log", "build/", "!keep.txt"},
		"Gemfile":          {`gem 'rails', '~> 7.0'`},
	}
	for file, lines := range yes {
		for _, l := range lines {
			if !isListLine(file, "\t"+l+"  ") {
				t.Errorf("%s: %q should be a list entry", file, l)
			}
		}
	}
	no := map[string][]string{
		"a.go":             {"func main() {}", "x := 1", "// import \"x\""},
		"a.py":             {"def f(): pass", "x = import_thing()"},
		"requirements.txt": {"", "def x"},
		"README.md":        {"import os", "flask==2.0"},
		"package.json":     {`"react": "^18.0.0",`},
	}
	for file, lines := range no {
		for _, l := range lines {
			if isListLine(file, l) {
				t.Errorf("%s: %q must not be a list entry", file, l)
			}
		}
	}
}

func TestWhitespaceEquality(t *testing.T) {
	if !wsEqual("a  b\n\n\nc\n", "a b\nc") || wsEqual("a b\nc", "a b\nd") || wsEqual("a\n", "a\nb\n") {
		t.Fatal("wsEqual")
	}
	if !wsEqual("f( a , b )", "f(a,b)") || wsEqual("a b", "ab") || !wsEqual("x  =  1", "x = 1") || !wsEqual("\t\treturn  x ;", "return x;") {
		t.Fatal("whitespace next to punctuation is insignificant, between words it is not")
	}
}
