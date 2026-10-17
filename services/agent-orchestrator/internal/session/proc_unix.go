//go:build unix

package session

import (
	"fmt"
	"os"
	"os/exec"
	"syscall"
)

// setProcessGroup puts the child in its own process group so the whole tree
// can be signalled.
func setProcessGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
}

func terminateGroup(pid int) { _ = syscall.Kill(-pid, syscall.SIGTERM) }
func killGroup(pid int)      { _ = syscall.Kill(-pid, syscall.SIGKILL) }

// exitDetails reports the exit status and the terminating signal, if any.
func exitDetails(ps *os.ProcessState) (code int, signal string) {
	if ws, ok := ps.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
		return -1, signalName(ws.Signal())
	}
	return ps.ExitCode(), ""
}

// signalName renders a signal the way operators write it.
func signalName(sig syscall.Signal) string {
	switch sig {
	case syscall.SIGKILL:
		return "SIGKILL"
	case syscall.SIGTERM:
		return "SIGTERM"
	case syscall.SIGINT:
		return "SIGINT"
	case syscall.SIGHUP:
		return "SIGHUP"
	case syscall.SIGSEGV:
		return "SIGSEGV"
	case syscall.SIGABRT:
		return "SIGABRT"
	case syscall.SIGBUS:
		return "SIGBUS"
	case syscall.SIGPIPE:
		return "SIGPIPE"
	}
	return fmt.Sprintf("signal %d", int(sig))
}
