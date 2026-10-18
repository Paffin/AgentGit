package testutil

import "syscall"

// setParentDeathSignal makes the kernel kill the server if the test binary dies.
func setParentDeathSignal(a *syscall.SysProcAttr) { a.Pdeathsig = syscall.SIGKILL }
