//go:build linux

package integration

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

// Shutdown in the default "wait" mode: no new session starts, the running one
// is allowed to finish (it is not signalled), its run is completed and its task
// finished, its credential is revoked, and /status keeps answering until the
// last session is done.
func TestShutdownWaitModeLetsRunningSessionsFinish(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"steady"}})
	dir := t.TempDir()
	sh := writeScript(t, "steady.sh", fmt.Sprintf(`touch %[1]s/started-$AG_TASK
printf '%%s' "$AG_TOKEN" > %[1]s/token-$AG_TASK
sleep 2
touch %[1]s/finished-$AG_TASK
`, dir))
	w.seed(map[string]string{"README.md": "x\n"})
	t1 := w.task("First", "", []string{"a/**"})
	time.Sleep(5 * time.Millisecond)
	t2 := w.task("Second", "", []string{"b/**"})

	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      slots: 1\n      default_executor: steady\n",
		Executors: commandExecutor("steady", sh, ""),
		Shutdown:  "  grace: 20s\n  mode: wait",
	})
	o.waitReady()
	eventually(t, "the first task to run", 30*time.Second, func() bool { return fileExists(filepath.Join(dir, "started-"+t1.ID)) })

	// Shut down while the session is running (what SIGTERM does).
	stopped := make(chan error, 1)
	began := time.Now()
	go func() { stopped <- o.stop() }()

	// The admin API stays up and shows the session draining.
	sawSession := false
	for time.Since(began) < 1500*time.Millisecond && !sawSession {
		code, body := o.get("/status")
		if code != 200 {
			t.Fatalf("/status during shutdown = %d %s", code, body)
		}
		sawSession = len(o.status().Sessions) == 1
		time.Sleep(50 * time.Millisecond)
	}
	if !sawSession {
		t.Error("/status did not show the session that is finishing")
	}
	select {
	case err := <-stopped:
		t.Fatalf("shutdown returned after %s (%v) before the running session finished", time.Since(began), err)
	default:
	}

	if err := <-stopped; err != nil {
		t.Fatalf("shutdown: %v", err)
	}
	if !fileExists(filepath.Join(dir, "finished-"+t1.ID)) {
		t.Error("the executor was cut short instead of being allowed to finish")
	}
	if fileExists(filepath.Join(dir, "started-"+t2.ID)) || len(w.runsOfTask(t2.ID)) != 0 {
		t.Error("a new session was started during shutdown")
	}
	if got := w.getTask(t1.ID); got.State != agwp.TaskDone {
		t.Errorf("task 1 = %+v", got)
	}
	if got := w.getTask(t2.ID); got.State != agwp.TaskOpen || got.Owner != "" {
		t.Errorf("task 2 must be left alone: %+v", got)
	}
	runs := w.runsOfTask(t1.ID)
	if len(runs) != 1 || runs[0].Status != "completed" {
		t.Errorf("runs of task 1 = %+v", runs)
	}
	if _, err := w.srv.client(readFile(t, filepath.Join(dir, "token-"+t1.ID)), "late").Whoami(context.Background()); !agwp.IsCode(err, agwp.CodeUnauthenticated) {
		t.Errorf("the session's credential still works after shutdown: %v", err)
	}
	if len(w.activeLeases()) != 0 {
		t.Errorf("leases left behind: %+v", w.activeLeases())
	}
	if c, err := net.DialTimeout("tcp", strings.TrimPrefix(o.base, "http://"), 300*time.Millisecond); err == nil {
		c.Close()
		t.Error("the admin listener is still open after shutdown")
	}
	if _, err := http.Get(o.base + "/healthz"); err == nil {
		t.Error("the admin API still answers after shutdown")
	}
}

// Shutdown in "cancel" mode: the executor is asked to stop (SIGTERM, then a
// kill after its grace), the run is recorded, the lease and credential are
// gone, and the task stays with its agent so that a restarted orchestrator
// adopts it and finishes the work.
func TestShutdownCancelModeStopsSessionsAndARestartAdoptsTheTask(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"patient"}})
	dir := t.TempDir()
	marker := filepath.Join(dir, "second-time")
	sh := writeScript(t, "patient.sh", fmt.Sprintf(`if [ -f %[2]s ]; then touch %[1]s/second-run-done; exit 0; fi
touch %[2]s
trap 'touch %[1]s/got-term; exit 0' TERM
touch %[1]s/started
printf '%%s' "$AG_TOKEN" > %[1]s/token
while true; do sleep 0.1; done
`, dir, marker))
	w.seed(map[string]string{"README.md": "x\n"})
	tk := w.task("Long job", "", []string{"lib/**"})
	state := t.TempDir()
	cfg := orchConfig{
		Repo:      "    schedule:\n      default_executor: patient\n",
		Executors: commandExecutor("patient", sh, ""),
		Shutdown:  "  grace: 10s\n  mode: cancel",
		StateDir:  state,
	}
	o := w.startOrchestrator(cfg)
	o.waitReady()
	eventually(t, "the executor to start", 30*time.Second, func() bool { return fileExists(filepath.Join(dir, "started")) })
	first := w.runOfTask(tk.ID)
	if first == nil || first.Status != "running" {
		t.Fatalf("run = %+v", first)
	}

	began := time.Now()
	if err := o.stop(); err != nil {
		t.Fatalf("shutdown: %v", err)
	}
	if d := time.Since(began); d > 8*time.Second {
		t.Errorf("shutdown took %s although the executor handles SIGTERM", d)
	}
	if !fileExists(filepath.Join(dir, "got-term")) {
		t.Error("the executor was not asked to stop with SIGTERM")
	}
	r := w.runOfTask(tk.ID)
	if r == nil || r.Status == "running" {
		t.Fatalf("the run is still running after shutdown: %+v", r)
	}
	if !strings.Contains(strings.ToLower(r.Summary), "cancel") && !strings.Contains(strings.ToLower(r.Summary), "shutdown") {
		t.Errorf("the run's summary should say why it ended: %q", r.Summary)
	}
	if got := w.getTask(tk.ID); got.State != agwp.TaskInProgress || got.Owner != "agent:patient" {
		t.Errorf("an interrupted task stays with its agent, got %+v", got)
	}
	if _, err := w.srv.client(readFile(t, filepath.Join(dir, "token")), "late").Whoami(context.Background()); !agwp.IsCode(err, agwp.CodeUnauthenticated) {
		t.Errorf("the credential survived the shutdown: %v", err)
	}
	if len(w.activeLeases()) != 0 {
		t.Errorf("leases left behind: %+v", w.activeLeases())
	}

	// A new orchestrator (same state directory, so the same agent key) adopts the task.
	o2 := w.startOrchestrator(cfg)
	o2.waitReady()
	eventually(t, "the restarted orchestrator to finish the task", 45*time.Second, func() bool { return w.getTask(tk.ID).State == agwp.TaskDone })
	if !fileExists(filepath.Join(dir, "second-run-done")) {
		t.Error("the second run did not happen")
	}
	if runs := w.runsOfTask(tk.ID); len(runs) != 2 {
		t.Errorf("runs = %+v", runs)
	}
}

// An executor that ignores SIGTERM is killed when its grace ends, with its whole
// process group, and shutdown does not hang.
func TestShutdownCancelKillsAnExecutorThatIgnoresTerm(t *testing.T) {
	w := newWorld(t, worldOptions{Agents: []string{"stubborn"}})
	dir := t.TempDir()
	sh := writeScript(t, "stubborn.sh", fmt.Sprintf(`trap '' TERM
( trap '' TERM; exec sleep 600 ) &
echo $! > %[1]s/child-pid
echo $$ > %[1]s/pid
while true; do sleep 0.1; done
`, dir))
	w.seed(map[string]string{"README.md": "x\n"})
	tk := w.task("Stubborn job", "", []string{"lib/**"})
	o := w.startOrchestrator(orchConfig{
		Repo:      "    schedule:\n      default_executor: stubborn\n",
		Executors: strings.Replace(commandExecutor("stubborn", sh, ""), "grace: 2s", "grace: 1s", 1),
		Shutdown:  "  grace: 10s\n  mode: cancel",
	})
	o.waitReady()
	eventually(t, "the executor to start", 30*time.Second, func() bool {
		return fileExists(filepath.Join(dir, "child-pid")) && fileExists(filepath.Join(dir, "pid"))
	})
	pid, _ := strconv.Atoi(readFile(t, filepath.Join(dir, "pid")))
	child, _ := strconv.Atoi(readFile(t, filepath.Join(dir, "child-pid")))

	began := time.Now()
	if err := o.stop(); err != nil {
		t.Fatalf("shutdown: %v", err)
	}
	if d := time.Since(began); d > 9*time.Second {
		t.Errorf("shutdown took %s", d)
	}
	for name, p := range map[string]int{"the executor": pid, "its child": child} {
		eventually(t, name+" to be gone", 5*time.Second, func() bool {
			return syscall.Kill(p, 0) == syscall.ESRCH || isZombie(p)
		})
	}
	if r := w.runOfTask(tk.ID); r == nil || r.Status == "running" {
		t.Errorf("run = %+v", r)
	}
	if len(w.activeLeases()) != 0 {
		t.Errorf("leases left behind: %+v", w.activeLeases())
	}
}

// isZombie: a killed process nobody has waited for yet still answers kill(0).
func isZombie(pid int) bool {
	b, err := os.ReadFile(fmt.Sprintf("/proc/%d/stat", pid))
	if err != nil {
		return true
	}
	s := string(b)
	i := strings.LastIndex(s, ")")
	return i >= 0 && len(s) > i+2 && s[i+2] == 'Z'
}
