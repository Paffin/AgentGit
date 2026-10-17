package session

import (
	"fmt"
	"sync"
)

// tail is an io.Writer that keeps only the last max bytes it was given.
// Executors can write gigabytes; the orchestrator keeps a bounded window.
type tail struct {
	mu    sync.Mutex
	max   int
	buf   []byte
	total int64
}

func newTail(max int) *tail { return &tail{max: max} }

// Write implements io.Writer and never fails.
func (t *tail) Write(p []byte) (int, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	t.total += int64(len(p))
	if len(p) >= t.max {
		t.buf = append(t.buf[:0], p[len(p)-t.max:]...)
		return len(p), nil
	}
	if over := len(t.buf) + len(p) - t.max; over > 0 {
		t.buf = append(t.buf[:0], t.buf[over:]...)
	}
	t.buf = append(t.buf, p...)
	return len(p), nil
}

// Total is the number of bytes ever written.
func (t *tail) Total() int64 {
	t.mu.Lock()
	defer t.mu.Unlock()
	return t.total
}

// Last returns up to n trailing bytes as text, marking truncation.
func (t *tail) Last(n int) string {
	t.mu.Lock()
	defer t.mu.Unlock()
	b := t.buf
	truncated := t.total > int64(len(b))
	if n > 0 && len(b) > n {
		b = b[len(b)-n:]
		truncated = true
	}
	s := string(b)
	// do not start in the middle of a UTF-8 sequence
	for i := 0; i < len(s) && i < 4; i++ {
		if s[i]&0xC0 != 0x80 {
			s = s[i:]
			break
		}
	}
	if truncated {
		return fmt.Sprintf("[... %d bytes omitted]\n%s", t.total-int64(len(s)), s)
	}
	return s
}
