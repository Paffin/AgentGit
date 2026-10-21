package testutil

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"sync"
	"testing"
)

// The one way a test of this repository says "the component I need is not here" (the Go twin of the Rust crate
// `ag-testkit`, the Python module `scripts/ag_testkit.py` and the TypeScript module `sdk/typescript/test/harness/
// components.ts`; see docs/architecture/test-skips.md).
//
// A test that talks to an external component (the ag-server binary, a model, ...) asks RequireComponent instead of
// returning early or calling t.Skip itself:
//
//   - the component is there: the test runs;
//   - the component is missing and AG_REQUIRE_COMPONENTS names it (or is "all"): the test FAILS;
//   - otherwise the test SKIPS and leaves a machine-readable marker, which scripts/check_skips.py compares with the
//     reviewed list tests/skip-allowlist.toml. The marker is the skip message ("AG-SKIP component=... test=...
//     reason=...") and, when AG_SKIP_REPORT names a file, one JSON line appended to it.
const (
	envRequire = "AG_REQUIRE_COMPONENTS"
	envReport  = "AG_SKIP_REPORT"
	skipMarker = "AG-SKIP"
)

// ListRequires reports whether a list ("a,b", "a b", "all") requires component. "oci" and "oci:docker" cover each
// other, like in the Rust crate.
func ListRequires(list, component string) bool {
	component = strings.ToLower(component)
	for _, t := range strings.FieldsFunc(list, func(r rune) bool { return r == ',' || r == ' ' || r == '\t' || r == '\n' }) {
		t = strings.ToLower(strings.TrimSpace(t))
		switch {
		case t == "":
		case t == "all", t == component:
			return true
		case strings.HasPrefix(component, t+":"), strings.HasPrefix(t, component+":"):
			return true
		}
	}
	return false
}

// Required reports whether the absence of component must fail a test in this process. The CI variable stands for
// "ag-server-binary" (the real-server tests must never silently skip in CI), as it always did.
func Required(component string) bool {
	if ListRequires(os.Getenv(envRequire), component) {
		return true
	}
	if component == "ag-server-binary" && (os.Getenv("CI") != "" || os.Getenv("AG_REQUIRE_SERVER") != "") {
		return true
	}
	return false
}

var reportMu sync.Mutex

func oneLine(s string) string { return strings.Join(strings.Fields(s), " ") }

// Marker is the text of a skip, the same line in every language.
func Marker(component, test, reason string) string {
	return fmt.Sprintf("%s component=%s test=%s reason=%s", skipMarker, component, test, oneLine(reason))
}

func record(component, test, reason string) {
	path := os.Getenv(envReport)
	if path == "" {
		return
	}
	line, _ := json.Marshal(map[string]string{"marker": skipMarker, "component": component, "test": test, "reason": oneLine(reason), "lang": "go"})
	reportMu.Lock()
	defer reportMu.Unlock()
	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	_, _ = f.Write(append(line, '\n'))
}

// RequireComponent runs on when err is nil. When it is not, the test fails if component is required and is
// skipped, with the marker, otherwise.
func RequireComponent(t testing.TB, component string, err error) {
	t.Helper()
	if err == nil {
		return
	}
	if Required(component) {
		t.Fatalf("%s is required (%s) but unavailable, so %s cannot run: %v", component, envRequire, t.Name(), err)
	}
	record(component, t.Name(), err.Error())
	t.Skip(Marker(component, t.Name(), err.Error()))
}

// SkipNotApplicable skips a test that does not apply under the configuration it runs in. Never a failure; the
// marker (component "n/a:<kind>") still reaches the report.
func SkipNotApplicable(t testing.TB, kind, reason string) {
	t.Helper()
	record("n/a:"+kind, t.Name(), reason)
	t.Skip(Marker("n/a:"+kind, t.Name(), reason))
}
