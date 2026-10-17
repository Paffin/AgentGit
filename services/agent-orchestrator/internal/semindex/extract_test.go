package semindex

import (
	"fmt"
	"sort"
	"strings"
	"testing"
)

// brief renders defs as "kind container.name line exported" for compact assertions.
func brief(defs []Def) []string {
	var out []string
	for _, d := range defs {
		e := "-"
		if d.Exported {
			e = "+"
		}
		out = append(out, fmt.Sprintf("%s%s %s %d", e, d.Kind, d.Qualified(), d.Line))
	}
	return out
}

func check(t *testing.T, lang, src string, want ...string) {
	t.Helper()
	got := brief(Extract(lang, []byte(src), nil).Defs)
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Errorf("%s definitions:\n%s\nwant:\n%s", lang, strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

func TestGoDefinitions(t *testing.T) {
	src := `package lib

import "fmt"

// Exported does a thing.
func Exported(a int) error { return nil }

func helper() {}

func (s *Server) Serve(ctx context.Context) error {
	return nil
}

func (s Server) quiet() {}

func (r *repo) Get() {}

func Generic[T any](x T) T { return x }

func (b *Box[T]) Put(x T) {}

type Config struct {
	Name string
}

type Reader interface {
	Read() error
}

type alias = int

type Handler func(int) error

const Version = "1"

var ErrNotFound = errors.New("x")

var (
	mu    sync.Mutex
	Cache = map[string]int{}
	_     = fmt.Sprint
)

const (
	StatusOK = iota
	StatusBad
)

type (
	Pair struct{ A, B int }
	Name string
)

/*
func inComment() {}
*/
func after() {}
`
	check(t, LangGo, src,
		"+func Exported 6", "-func helper 8", "+method Server.Serve 10", "-method Server.quiet 14", "-method repo.Get 16",
		"+func Generic 18", "+method Box.Put 20", "+struct Config 22", "+interface Reader 26", "-type alias 30", "+type Handler 32",
		"+const Version 34", "+var ErrNotFound 36", "-var mu 39", "+var Cache 40", "+const StatusOK 45", "+const StatusBad 46",
		"+struct Pair 50", "+type Name 51", "-func after 57")
}

func TestRustDefinitions(t *testing.T) {
	src := `use std::fmt;

pub fn public_fn(x: u32) -> u32 { x }

fn private_fn() {}

pub(crate) fn crate_fn() {}

pub async fn fetch() {}

pub struct Config {
    pub name: String,
}

enum Mode { A, B }

pub trait Store {
    fn get(&self);
}

impl Config {
    pub fn new() -> Self { Self }
    fn secret(&self) {}
}

impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result { Ok(()) }
}

pub const MAX: usize = 4;
static COUNTER: AtomicUsize = AtomicUsize::new(0);
pub mod util;
pub type Result<T> = std::result::Result<T, Error>;

macro_rules! helper {
    () => {};
}

// fn commented() {}

fn last() {}
`
	check(t, LangRust, src,
		"+func public_fn 3", "-func private_fn 5", "-func crate_fn 7", "+func fetch 9", "+struct Config 11", "-enum Mode 15",
		"+trait Store 17", "-func get 18", "+method Config.new 22", "-method Config.secret 23", "-method Config.fmt 27",
		"+const MAX 30", "-static COUNTER 31", "+mod util 32", "+type Result 33", "+macro helper 35", "-func last 41")
}

func TestPythonDefinitions(t *testing.T) {
	src := `"""Module docstring.

def not_a_function():
    pass
"""
import os

MAX_RETRIES = 3
_PRIVATE = 1
lowercase = 5

def public(a, b):
    x = 1
    def nested():
        pass
    return x

async def fetch():
    pass

def _helper():
    pass

class Service:
    TIMEOUT = 30

    def __init__(self):
        self.x = 1

    def run(self):
        def inner():
            pass
        return inner

    async def stop(self):
        pass

    class Meta:
        def inside(self):
            pass

def after_class():
    pass
`
	check(t, LangPython, src,
		"+const MAX_RETRIES 8", "-const _PRIVATE 9", "+func public 12", "+func fetch 18", "-func _helper 21", "+class Service 24",
		"-method Service.__init__ 27", "+method Service.run 30", "+method Service.stop 35", "+class Service.Meta 38",
		"+method Meta.inside 39", "+func after_class 42")
}

func TestTypeScriptAndJavaScriptDefinitions(t *testing.T) {
	src := `import { a } from "./a";

export function exported(x: number): number {
  return x;
}

function local() {}

export async function load() {}

export default class Widget extends Base {
  private secret = 1;

  render(): string {
    return "x";
  }

  private hidden() {
    if (x) {
      return;
    }
  }

  static create<T>(x: T): Widget {
    return new Widget();
  }

  get value(): number {
    return 1;
  }
}

class Internal {
  go() {}
}

export interface Options {
  verbose: boolean;
}

export type Id = string;

enum Level { Low, High }

export const LIMIT = 10;

const handler = (req) => {
  return 1;
};

export let counter: number = 0;

const x = require("x");
`
	check(t, LangTS, src,
		"+func exported 3", "-func local 7", "+func load 9", "+class Widget 11", "+method Widget.render 14", "-method Widget.hidden 18",
		"+method Widget.create 24", "+method Widget.value 28", "-class Internal 33", "-method Internal.go 34", "+interface Options 37",
		"+type Id 41", "-enum Level 43", "+const LIMIT 45", "-const handler 47", "+let counter 51", "-const x 53")
}

func TestJavaScriptUsesTheSameScanner(t *testing.T) {
	if LanguageOf("a/b.jsx") != LangJS || LanguageOf("a/b.mjs") != LangJS || LanguageOf("x.TSX") != LangTS {
		t.Fatal("LanguageOf")
	}
	check(t, LangJS, "function a() {}\nmodule.exports = a;\n", "-func a 1")
}

func TestLanguageOf(t *testing.T) {
	for p, want := range map[string]string{"a.go": LangGo, "x/y.rs": LangRust, "z.py": LangPython, "q.ts": LangTS, "README.md": "", "Makefile": "", "a.go.bak": ""} {
		if got := LanguageOf(p); got != want {
			t.Errorf("LanguageOf(%q) = %q, want %q", p, got, want)
		}
	}
}

func TestSpansRunToTheNextDefinition(t *testing.T) {
	s := Extract(LangGo, []byte("package a\n\nfunc A() {\n\t_ = 1\n}\n\nfunc B() {}\n"), nil)
	if len(s.Defs) != 2 || s.Defs[0].Line != 3 || s.Defs[0].EndLine != 6 || s.Defs[1].Line != 7 || s.Defs[1].EndLine != 8 {
		t.Fatalf("defs = %+v", s.Defs)
	}
}

func TestIdentifierCountsAreBoundedAndPreferKnownNames(t *testing.T) {
	var b strings.Builder
	for i := 0; i < 3000; i++ {
		fmt.Fprintf(&b, "word%04d ", i)
	}
	b.WriteString("rareButDefined rareButDefined\n")
	for i := 0; i < 600; i++ {
		b.WriteString("common common common ") // frequent but unknown
	}
	s := Extract(LangGo, []byte(b.String()), map[string]bool{"rareButDefined": true})
	if len(s.Idents) != maxIdentsPerFile {
		t.Fatalf("idents = %d, want the cap of %d", len(s.Idents), maxIdentsPerFile)
	}
	if s.Idents["rareButDefined"] != 2 {
		t.Fatalf("a name defined elsewhere must be kept: %v", s.Idents["rareButDefined"])
	}
	if s.Idents["common"] != 1800 {
		t.Fatalf("frequent names come next: %d", s.Idents["common"])
	}
	if _, kw := s.Idents["func"]; kw {
		t.Fatal("keywords are not identifiers")
	}
	// counts saturate instead of wrapping
	big := Extract(LangGo, []byte(strings.Repeat("overflow ", 70000)), nil)
	if big.Idents["overflow"] != 65535 {
		t.Fatalf("count = %d", big.Idents["overflow"])
	}
}

func TestExtractionIsBoundedPerFile(t *testing.T) {
	var b strings.Builder
	for i := 0; i < 2000; i++ {
		fmt.Fprintf(&b, "func F%d() {}\n", i)
	}
	if n := len(Extract(LangGo, []byte(b.String()), nil).Defs); n != maxDefsPerFile {
		t.Fatalf("defs = %d", n)
	}
}

func TestCRLFAndGarbageDoNotPanic(t *testing.T) {
	check(t, LangGo, "package a\r\n\r\nfunc A() {}\r\n", "+func A 3")
	for _, lang := range []string{LangGo, LangRust, LangPython, LangTS, LangJS} {
		for _, junk := range []string{"", "\x00\x01\x02", strings.Repeat("(", 10000), "func", "fn", "def", "class", "export", "type (\n", "impl<"} {
			_ = Extract(lang, []byte(junk), nil)
		}
	}
}

func TestDefinitionsComeInFileOrderAndAreDeterministic(t *testing.T) {
	src := "package a\n\nfunc B() {}\nfunc A() {}\nvar C = 1\n"
	first := brief(Extract(LangGo, []byte(src), nil).Defs)
	if !sort.SliceIsSorted(first, func(i, j int) bool { return i < j }) || first[0] != "+func B 3" {
		t.Fatalf("defs = %v", first)
	}
	for i := 0; i < 10; i++ {
		if strings.Join(brief(Extract(LangGo, []byte(src), nil).Defs), ",") != strings.Join(first, ",") {
			t.Fatal("non-deterministic")
		}
	}
}
