//go:build unix && !linux

package testutil

import "syscall"

func setParentDeathSignal(*syscall.SysProcAttr) {}
