package testutil

import (
	"context"
	"fmt"
	"strings"
	"testing"
)

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
