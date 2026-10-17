package metrics

import (
	"fmt"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
)

func render(r *Registry) string {
	var b strings.Builder
	r.WriteText(&b)
	return b.String()
}

func TestCounterVecRendersSortedLabelledSeries(t *testing.T) {
	r := NewRegistry()
	c := r.NewCounterVec("jobs_total", "Jobs done.", "repo", "result")
	c.With("b/r", "ok").Add(2)
	c.With("a/r", "fail").Inc()
	c.With("a/r", "ok").Inc()
	c.With("a/r", "ok").Inc()
	got := render(r)
	want := `# HELP jobs_total Jobs done.
# TYPE jobs_total counter
jobs_total{repo="a/r",result="fail"} 1
jobs_total{repo="a/r",result="ok"} 2
jobs_total{repo="b/r",result="ok"} 2
`
	if got != want {
		t.Fatalf("got:\n%s\nwant:\n%s", got, want)
	}
}

func TestCountersNeverDecrease(t *testing.T) {
	var c Counter
	c.Add(5)
	c.Add(-3)
	c.Add(0)
	if c.Value() != 5 {
		t.Fatalf("value = %v", c.Value())
	}
}

func TestUnlabelledMetricsExposeZeroSamples(t *testing.T) {
	r := NewRegistry()
	r.NewCounterVec("plain_total", "x")
	r.NewGaugeVec("plain_gauge", "y")
	got := render(r)
	for _, want := range []string{"plain_total 0\n", "plain_gauge 0\n"} {
		if !strings.Contains(got, want) {
			t.Errorf("missing %q in:\n%s", want, got)
		}
	}
}

func TestGaugeAndGaugeFunc(t *testing.T) {
	r := NewRegistry()
	g := r.NewGaugeVec("running", "Running.", "executor")
	g.With("x").Inc()
	g.With("x").Inc()
	g.With("x").Dec()
	g.With("y").Set(7.5)
	n := 3.0
	r.NewGaugeFunc("uptime", "Uptime.", func() float64 { return n })
	out := render(r)
	for _, want := range []string{`running{executor="x"} 1`, `running{executor="y"} 7.5`, "uptime 3", "# TYPE uptime gauge"} {
		if !strings.Contains(out, want) {
			t.Errorf("missing %q in:\n%s", want, out)
		}
	}
	g.Reset()
	if strings.Contains(render(r), `running{`) {
		t.Fatal("Reset must drop series")
	}
}

func TestLabelEscaping(t *testing.T) {
	r := NewRegistry()
	c := r.NewCounterVec("esc_total", "h", "v")
	c.With("a\"b\\c\nd").Inc()
	if !strings.Contains(render(r), `esc_total{v="a\"b\\c\nd"} 1`) {
		t.Fatalf("bad escaping:\n%s", render(r))
	}
}

func TestSeriesAreBounded(t *testing.T) {
	r := NewRegistry()
	c := r.NewCounterVec("cardinality_total", "h", "v")
	for i := 0; i < maxSeries+50; i++ {
		c.With(fmt.Sprint(i)).Inc()
	}
	out := render(r)
	if !strings.Contains(out, `cardinality_total{v="overflow"} 50`) {
		t.Fatalf("expected overflow series with 50, got tail:\n%s", out[max(0, len(out)-200):])
	}
	if lines := strings.Count(out, "cardinality_total{"); lines != maxSeries+1 {
		t.Fatalf("series = %d, want %d", lines, maxSeries+1)
	}
}

func TestConcurrentUse(t *testing.T) {
	r := NewRegistry()
	c := r.NewCounterVec("conc_total", "h", "k")
	var wg sync.WaitGroup
	for i := 0; i < 8; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for j := 0; j < 1000; j++ {
				c.With("a").Inc()
			}
		}()
	}
	wg.Wait()
	if v := c.With("a").Value(); v != 8000 {
		t.Fatalf("value = %v", v)
	}
}

func TestInvalidAndDuplicateNamesPanic(t *testing.T) {
	r := NewRegistry()
	r.NewCounterVec("ok_total", "h")
	for name, f := range map[string]func(){
		"duplicate": func() { r.NewCounterVec("ok_total", "h") },
		"invalid":   func() { r.NewGaugeVec("bad name", "h") },
		"arity":     func() { r.NewCounterVec("arity_total", "h", "a").With("x", "y") },
	} {
		func() {
			defer func() {
				if recover() == nil {
					t.Errorf("%s: expected panic", name)
				}
			}()
			f()
		}()
	}
}

func TestHandlerContentType(t *testing.T) {
	r := NewRegistry()
	r.NewCounterVec("h_total", "h")
	rec := httptest.NewRecorder()
	r.Handler().ServeHTTP(rec, httptest.NewRequest("GET", "/metrics", nil))
	if ct := rec.Header().Get("Content-Type"); !strings.HasPrefix(ct, "text/plain; version=0.0.4") {
		t.Fatalf("content type %q", ct)
	}
	if !strings.Contains(rec.Body.String(), "h_total 0") {
		t.Fatalf("body %q", rec.Body.String())
	}
}

func TestServiceCatalogueRegisters(t *testing.T) {
	s := NewService(NewRegistry())
	s.SessionsStarted.With("a/b", "x").Inc()
	s.APIErrors.With("not_found").Inc()
	out := render(s.Registry)
	if !strings.Contains(out, `orchestrator_sessions_started_total{repo="a/b",executor="x"} 1`) {
		t.Fatalf("missing sessions counter:\n%s", out)
	}
	if !strings.Contains(out, `orchestrator_agwp_errors_total{code="not_found"} 1`) {
		t.Fatalf("missing error counter:\n%s", out)
	}
}

func TestFormatFloat(t *testing.T) {
	for in, want := range map[float64]string{0: "0", 1: "1", 1.5: "1.5", 1e21: "1e+21"} {
		if got := formatFloat(in); got != want {
			t.Errorf("formatFloat(%v) = %q, want %q", in, got, want)
		}
	}
}
