package fakeagwp

import (
	"bytes"
	"fmt"
	"sort"
	"strings"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/textdiff"
)

// diffFiles renders per-file unified diffs with zero context lines, in the
// shape the real server answers with.
func diffFiles(old, new map[string][]byte) []agwp.FileDiff {
	paths := map[string]bool{}
	for p := range old {
		paths[p] = true
	}
	for p := range new {
		paths[p] = true
	}
	sorted := make([]string, 0, len(paths))
	for p := range paths {
		sorted = append(sorted, p)
	}
	sort.Strings(sorted)
	out := []agwp.FileDiff{}
	for _, p := range sorted {
		o, hadOld := old[p]
		n, hasNew := new[p]
		if hadOld && hasNew && bytes.Equal(o, n) {
			continue
		}
		fd := agwp.FileDiff{Path: p}
		switch {
		case !hadOld:
			fd.Status = "added"
		case !hasNew:
			fd.Status = "deleted"
		default:
			fd.Status = "modified"
		}
		if bytes.IndexByte(o, 0) >= 0 || bytes.IndexByte(n, 0) >= 0 {
			fd.Binary = true
			out = append(out, fd)
			continue
		}
		a, b := textdiff.SplitLines(string(o)), textdiff.SplitLines(string(n))
		var sb strings.Builder
		fmt.Fprintf(&sb, "--- a/%s\n+++ b/%s\n", p, p)
		for _, h := range textdiff.Diff(a, b) {
			fmt.Fprintf(&sb, "@@ -%d,%d +%d,%d @@\n", start(h.AStart, h.AEnd), h.AEnd-h.AStart, start(h.BStart, h.BEnd), h.BEnd-h.BStart)
			for _, l := range a[h.AStart:h.AEnd] {
				sb.WriteString("-" + strings.TrimSuffix(l, "\n") + "\n")
				fd.Deletions++
			}
			for _, l := range b[h.BStart:h.BEnd] {
				sb.WriteString("+" + strings.TrimSuffix(l, "\n") + "\n")
				fd.Additions++
			}
		}
		fd.Patch = sb.String()
		out = append(out, fd)
	}
	return out
}

// start follows the unified-diff convention: a zero-length range is addressed
// by the line before it.
func start(from, to int) int {
	if to == from {
		return from
	}
	return from + 1
}
