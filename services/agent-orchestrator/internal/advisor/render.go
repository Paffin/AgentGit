package advisor

import (
	"fmt"
	"strings"
)

// MarkerPrefix starts the machine-readable line every advisor comment carries;
// it is how the advisor recognises its own earlier comments.
const MarkerPrefix = "ag-advisor/1 digest="

// maxComment bounds one review body.
const maxComment = 60 << 10

// Render writes the review body for a proposal's suggestions. It is plain
// Markdown. inline is the largest resolved file included verbatim.
func Render(advisorName, proposalHead, streamHead, stream string, sugs []Suggestion, inline int, digest string) string {
	var b strings.Builder
	fmt.Fprintf(&b, "**Conflict advisor** — advice only (%s)\n\n", advisorName)
	fmt.Fprintf(&b, "The proposal (head `%s`) conflicts with stream `%s` (head `%s`) in %d file(s). ", short(proposalHead), stream, short(streamHead), len(sugs))
	b.WriteString("Nothing below is applied automatically and none of it is validated: a resolution only enters the repository when an agent or a person submits it " +
		"(workspace sync with `resolutions`), and then validation, approvals and policy decide whether it lands.\n")
	truncated := false
	for _, s := range sugs {
		section := renderOne(s, inline)
		if b.Len()+len(section) > maxComment {
			truncated = true
			break
		}
		b.WriteString(section)
	}
	if truncated {
		b.WriteString("\n_Further files are omitted to keep this comment small; the advisor's status API has the complete suggestions._\n")
	}
	fmt.Fprintf(&b, "\n<!-- %s%s -->\n", MarkerPrefix, digest)
	return b.String()
}

func renderOne(s Suggestion, inline int) string {
	var b strings.Builder
	fmt.Fprintf(&b, "\n### `%s` — %s\n", s.Path, describeClass(s))
	if s.Kind != "" {
		fmt.Fprintf(&b, "Reported by the server as: %s.\n", s.Kind)
	}
	for _, n := range s.Notes {
		fmt.Fprintf(&b, "- %s\n", n)
	}
	for i, r := range s.Regions {
		if len(s.Regions) > 1 || r.Class != s.Class {
			fmt.Fprintf(&b, "\nRegion %d (base lines %s): **%s**", i+1, lineSpan(r), r.Class)
			if r.Resolution != "none" {
				fmt.Fprintf(&b, ", resolution `%s`, confidence %s", r.Resolution, r.Confidence)
			}
			b.WriteString("\n")
		}
		fmt.Fprintf(&b, "%s\n", r.Why)
	}
	switch {
	case s.Complete && len(s.Resolved) > 0 && len(s.Resolved) <= inline:
		fmt.Fprintf(&b, "\nSuggested file (sha256 `%s`, confidence %s):\n\n", short(s.SHA256), s.Confidence)
		b.WriteString(fence(string(s.Resolved)))
	case s.Complete:
		fmt.Fprintf(&b, "\nA complete suggested file exists (sha256 `%s`, %d bytes, confidence %s) but is too large to inline; fetch it from the advisor's status API.\n", short(s.SHA256), len(s.Resolved), s.Confidence)
	}
	if !s.Complete && s.Class != ClassBinary && s.Class != ClassDeleteEdit {
		for i, r := range s.Regions {
			if r.Resolution != "none" && r.Proposed != "" {
				fmt.Fprintf(&b, "\nProposed text for region %d:\n\n%s", i+1, fence(r.Proposed))
			}
		}
	}
	return b.String()
}

func describeClass(s Suggestion) string {
	switch s.Class {
	case ClassWhitespace:
		return "whitespace-only difference"
	case ClassUnion:
		return "both sides extended the same list"
	case ClassAdjacent:
		return "neighbouring edits"
	case ClassSameLines:
		return "both sides added the same lines"
	case ClassMixed:
		return "several kinds of conflict"
	case ClassBothAdded:
		return "both sides created the file"
	case ClassDeleteEdit:
		return "deleted on one side, changed on the other"
	case ClassBinary:
		return "not analysed"
	}
	return "needs a person"
}

func lineSpan(r Region) string {
	if r.BaseEnd < r.BaseStart {
		return fmt.Sprintf("insertion before line %d", r.BaseStart)
	}
	if r.BaseStart == r.BaseEnd {
		return fmt.Sprintf("%d", r.BaseStart)
	}
	return fmt.Sprintf("%d-%d", r.BaseStart, r.BaseEnd)
}

// fence wraps text in a code fence longer than any backtick run inside it.
func fence(text string) string {
	longest, run := 0, 0
	for _, r := range text {
		if r == '`' {
			run++
			if run > longest {
				longest = run
			}
		} else {
			run = 0
		}
	}
	f := strings.Repeat("`", max(3, longest+1))
	if !strings.HasSuffix(text, "\n") {
		text += "\n"
	}
	return f + "\n" + text + f + "\n"
}

func short(s string) string {
	if len(s) > 12 {
		return s[:12]
	}
	return s
}
