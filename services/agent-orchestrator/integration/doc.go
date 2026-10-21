// Package integration holds the end-to-end tests of the orchestrator against
// the real ag-server binary. It contains only tests: see harness_test.go for
// how the server is started and scenarios_test.go for what is demonstrated.
//
// The tests need the server binary. They look at $AG_SERVER_BIN, then at
// "ag-server" on PATH, then in the cargo target directory of the repository
// (wherever cargo is configured to put it; internal/testutil.FindServerBin).
// When none exists they print the exact build command and skip with an AG-SKIP
// marker, or FAIL when AG_REQUIRE_COMPONENTS names ag-server-binary (CI sets
// it). Every test starts its own server on a free loopback port in a temporary
// data directory and kills it when it ends, pass or fail.
package integration
