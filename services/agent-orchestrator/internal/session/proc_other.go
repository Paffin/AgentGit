//go:build !unix

package session

import (
	"os"
	"os/exec"
)

// On platforms without process groups only the direct child can be stopped.
func setProcessGroup(*exec.Cmd) {}

func terminateGroup(pid int) {
	if p, err := os.FindProcess(pid); err == nil {
		_ = p.Kill()
	}
}

func killGroup(pid int) { terminateGroup(pid) }

func exitDetails(ps *os.ProcessState) (code int, signal string) { return ps.ExitCode(), "" }
