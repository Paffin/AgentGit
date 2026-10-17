// Package metrics is a tiny, dependency-free implementation of the parts of
// the Prometheus data model this service needs: labelled counters, labelled
// gauges and gauge functions, rendered in the text exposition format.
package metrics

import (
	"fmt"
	"io"
	"math"
	"net/http"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
)

// maxSeries bounds the number of label combinations per metric so a bug (or a
// hostile value) cannot grow memory without limit. Further combinations are
// folded into one "overflow" series.
const maxSeries = 1000

var nameRE = regexp.MustCompile(`^[a-zA-Z_:][a-zA-Z0-9_:]*$`)

// Registry owns a set of metrics and renders them.
type Registry struct {
	mu      sync.Mutex
	metrics map[string]collector
}

type collector interface {
	write(w io.Writer)
	help() string
	kind() string
}

// NewRegistry returns an empty registry.
func NewRegistry() *Registry { return &Registry{metrics: map[string]collector{}} }

func (r *Registry) register(name string, c collector) {
	if !nameRE.MatchString(name) {
		panic("metrics: invalid metric name " + strconv.Quote(name))
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, dup := r.metrics[name]; dup {
		panic("metrics: duplicate metric " + name)
	}
	r.metrics[name] = c
}

// WriteText renders all metrics in the Prometheus text format, sorted by name.
func (r *Registry) WriteText(w io.Writer) {
	r.mu.Lock()
	names := make([]string, 0, len(r.metrics))
	for n := range r.metrics {
		names = append(names, n)
	}
	sort.Strings(names)
	cs := make([]collector, len(names))
	for i, n := range names {
		cs[i] = r.metrics[n]
	}
	r.mu.Unlock()
	for i, n := range names {
		fmt.Fprintf(w, "# HELP %s %s\n# TYPE %s %s\n", n, escapeHelp(cs[i].help()), n, cs[i].kind())
		cs[i].write(w)
	}
}

// Handler serves the text exposition format.
func (r *Registry) Handler() http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
		r.WriteText(w)
	})
}

// Counter is a monotonically increasing value.
type Counter struct{ bits atomic.Uint64 }

// Inc adds one.
func (c *Counter) Inc() { c.Add(1) }

// Add adds v (negative values are ignored: counters never decrease).
func (c *Counter) Add(v float64) {
	if v <= 0 || math.IsNaN(v) {
		return
	}
	for {
		old := c.bits.Load()
		if c.bits.CompareAndSwap(old, math.Float64bits(math.Float64frombits(old)+v)) {
			return
		}
	}
}

// Value returns the current value.
func (c *Counter) Value() float64 { return math.Float64frombits(c.bits.Load()) }

// Gauge is a value that can go up and down.
type Gauge struct{ bits atomic.Uint64 }

// Set stores v.
func (g *Gauge) Set(v float64) { g.bits.Store(math.Float64bits(v)) }

// Add adds v (which may be negative).
func (g *Gauge) Add(v float64) {
	for {
		old := g.bits.Load()
		if g.bits.CompareAndSwap(old, math.Float64bits(math.Float64frombits(old)+v)) {
			return
		}
	}
}

// Inc adds one.
func (g *Gauge) Inc() { g.Add(1) }

// Dec subtracts one.
func (g *Gauge) Dec() { g.Add(-1) }

// Value returns the current value.
func (g *Gauge) Value() float64 { return math.Float64frombits(g.bits.Load()) }

type vec[T any] struct {
	name   string
	helpS  string
	kindS  string
	labels []string
	mu     sync.Mutex
	series map[string]*entry[T]
	newT   func() *T
	val    func(*T) float64
}

type entry[T any] struct {
	values []string
	m      *T
}

func (v *vec[T]) with(values ...string) *T {
	if len(values) != len(v.labels) {
		panic(fmt.Sprintf("metrics: %s expects %d label values, got %d", v.name, len(v.labels), len(values)))
	}
	key := strings.Join(values, "\xff")
	v.mu.Lock()
	defer v.mu.Unlock()
	if e, ok := v.series[key]; ok {
		return e.m
	}
	if len(v.series) >= maxSeries {
		values = make([]string, len(v.labels))
		for i := range values {
			values[i] = "overflow"
		}
		key = strings.Join(values, "\xff")
		if e, ok := v.series[key]; ok {
			return e.m
		}
	}
	e := &entry[T]{values: append([]string(nil), values...), m: v.newT()}
	v.series[key] = e
	return e.m
}

func (v *vec[T]) help() string { return v.helpS }
func (v *vec[T]) kind() string { return v.kindS }

func (v *vec[T]) write(w io.Writer) {
	v.mu.Lock()
	keys := make([]string, 0, len(v.series))
	for k := range v.series {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	type row struct {
		labels string
		val    float64
	}
	rows := make([]row, 0, len(keys))
	for _, k := range keys {
		e := v.series[k]
		rows = append(rows, row{labels: renderLabels(v.labels, e.values), val: v.val(e.m)})
	}
	v.mu.Unlock()
	if len(v.labels) == 0 && len(rows) == 0 {
		fmt.Fprintf(w, "%s 0\n", v.name)
	}
	for _, r := range rows {
		fmt.Fprintf(w, "%s%s %s\n", v.name, r.labels, formatFloat(r.val))
	}
}

// CounterVec is a family of counters distinguished by label values.
type CounterVec struct{ v *vec[Counter] }

// NewCounterVec registers a counter family. With no labels it behaves as one counter.
func (r *Registry) NewCounterVec(name, help string, labels ...string) *CounterVec {
	v := &vec[Counter]{name: name, helpS: help, kindS: "counter", labels: labels, series: map[string]*entry[Counter]{},
		newT: func() *Counter { return &Counter{} }, val: func(c *Counter) float64 { return c.Value() }}
	r.register(name, v)
	cv := &CounterVec{v}
	if len(labels) == 0 {
		cv.With() // expose a zero sample from the start
	}
	return cv
}

// With returns the counter for the given label values.
func (c *CounterVec) With(values ...string) *Counter { return c.v.with(values...) }

// GaugeVec is a family of gauges distinguished by label values.
type GaugeVec struct{ v *vec[Gauge] }

// NewGaugeVec registers a gauge family.
func (r *Registry) NewGaugeVec(name, help string, labels ...string) *GaugeVec {
	v := &vec[Gauge]{name: name, helpS: help, kindS: "gauge", labels: labels, series: map[string]*entry[Gauge]{},
		newT: func() *Gauge { return &Gauge{} }, val: func(g *Gauge) float64 { return g.Value() }}
	r.register(name, v)
	gv := &GaugeVec{v}
	if len(labels) == 0 {
		gv.With()
	}
	return gv
}

// With returns the gauge for the given label values.
func (g *GaugeVec) With(values ...string) *Gauge { return g.v.with(values...) }

// Reset forgets every series (used by gauges recomputed from scratch on each scrape).
func (g *GaugeVec) Reset() {
	g.v.mu.Lock()
	g.v.series = map[string]*entry[Gauge]{}
	g.v.mu.Unlock()
}

type gaugeFunc struct {
	name, helpS string
	fn          func() float64
}

func (g gaugeFunc) help() string { return g.helpS }
func (g gaugeFunc) kind() string { return "gauge" }
func (g gaugeFunc) write(w io.Writer) {
	fmt.Fprintf(w, "%s %s\n", g.name, formatFloat(g.fn()))
}

// NewGaugeFunc registers a gauge whose value is computed on every scrape.
func (r *Registry) NewGaugeFunc(name, help string, fn func() float64) {
	r.register(name, gaugeFunc{name: name, helpS: help, fn: fn})
}

func renderLabels(names, values []string) string {
	if len(names) == 0 {
		return ""
	}
	var b strings.Builder
	b.WriteByte('{')
	for i, n := range names {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteString(n)
		b.WriteString(`="`)
		b.WriteString(escapeLabel(values[i]))
		b.WriteByte('"')
	}
	b.WriteByte('}')
	return b.String()
}

func escapeLabel(s string) string {
	return strings.NewReplacer(`\`, `\\`, `"`, `\"`, "\n", `\n`).Replace(s)
}

func escapeHelp(s string) string {
	return strings.NewReplacer(`\`, `\\`, "\n", `\n`).Replace(s)
}

func formatFloat(v float64) string {
	switch {
	case math.IsInf(v, 1):
		return "+Inf"
	case math.IsInf(v, -1):
		return "-Inf"
	case math.IsNaN(v):
		return "NaN"
	}
	return strconv.FormatFloat(v, 'g', -1, 64)
}
