package testutil

import (
	"context"
	"fmt"
	"strings"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

func TestParallelServersAllocateTheirOwnPorts(t *testing.T) {
	for i := 0; i < 8; i++ {
		t.Run(fmt.Sprint(i), func(t *testing.T) {
			t.Parallel()
			s := StartServerWith(t, ServerOptions{TLS: i%2 == 1, Env: []string{"RUST_LOG=error"}})
			if !strings.Contains(strings.Join(s.cmd.Args, " "), "--listen 127.0.0.1:0") {
				t.Fatal("server must bind its own ephemeral port")
			}
			if strings.HasSuffix(s.URL, ":0") {
				t.Fatal("bound port was not discovered")
			}
			if _, err := s.Admin.Whoami(context.Background()); err != nil {
				t.Fatal(err)
			}
		})
	}
}

func TestListeningAddressRequiresACompleteListeningEvent(t *testing.T) {
	for _, c := range []struct{ log, want string }{
		{"binding 127.0.0.1:5555\n", ""},
		{"INFO listening (plaintext: HTTP + h2c gRPC) addr=127.0.0.1:42", ""},
		{"INFO listening (plaintext: HTTP + h2c gRPC) addr=127.0.0.1:42000\n", "127.0.0.1:42000"},
		{"INFO listening (TLS: HTTPS + gRPC) addr=127.0.0.1:42001\n", "127.0.0.1:42001"},
	} {
		if got := listeningAddress([]byte(c.log)); got != c.want {
			t.Errorf("listeningAddress(%q) = %q, want %q", c.log, got, c.want)
		}
	}
}

// Another real server can answer public capability probes but cannot accept this child's secret.
func TestReadinessRequiresTheChildCredential(t *testing.T) {
	foreign := StartServer(t)
	wrong, err := agwp.New(foreign.URL, secret.New(RandHex(24)))
	if err != nil {
		t.Fatal(err)
	}
	if _, err := wrong.Negotiate(context.Background()); err != nil {
		t.Fatal(err)
	}
	candidate := &Server{Admin: wrong, done: make(chan struct{})}
	if candidate.waitReady(150 * time.Millisecond) {
		t.Fatal("a public capability response must not make an unrelated server ready")
	}
}
