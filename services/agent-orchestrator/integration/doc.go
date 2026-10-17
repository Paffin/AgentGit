// Package integration holds the end-to-end tests of the orchestrator against
// the real ag-server binary. It contains only tests: see harness_test.go for
// how the server is started and scenarios_test.go for what is demonstrated.
//
// The tests need the server binary. They look at $AG_SERVER_BIN, then at
// "ag-server" on PATH, then at /home/user/bin/ag-server, and skip (loudly)
// when none exists. Every test starts its own server on a free loopback port
// in a temporary data directory and kills it when it ends, pass or fail.
package integration
