//go:build linux

package session

import (
	"os/exec"
	"testing"
	"time"
)

func TestProcessExitedDistinguishesRunningAndUnreapedChildren(t *testing.T) {
	cmd := exec.Command("sleep", "60")
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
	}()
	pid := cmd.Process.Pid
	if processExited(pid) {
		t.Fatal("a running child was treated as exited")
	}
	if err := cmd.Process.Kill(); err != nil {
		t.Fatal(err)
	}
	// Deliberately do not Wait yet: the child remains a zombie owned by this test.
	deadline := time.Now().Add(5 * time.Second)
	for !processExited(pid) && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if !processExited(pid) {
		t.Fatal("a killed, unreaped child was treated as running")
	}
}
