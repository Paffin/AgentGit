package llm

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// ErrNoJSON: the reply contains no JSON object.
var ErrNoJSON = errors.New("llm: the reply contains no JSON object")

// ExtractJSON returns the first top-level JSON object in a model reply. Models wrap their answer in prose or in a
// Markdown fence even when told not to; both are tolerated, nothing else is guessed.
func ExtractJSON(text string) (string, error) {
	start := strings.IndexByte(text, '{')
	if start < 0 {
		return "", ErrNoJSON
	}
	depth, inStr, esc := 0, false, false
	for i := start; i < len(text); i++ {
		c := text[i]
		switch {
		case esc:
			esc = false
		case inStr:
			switch c {
			case '\\':
				esc = true
			case '"':
				inStr = false
			}
		default:
			switch c {
			case '"':
				inStr = true
			case '{':
				depth++
			case '}':
				depth--
				if depth == 0 {
					return text[start : i+1], nil
				}
			}
		}
	}
	return "", fmt.Errorf("%w (the object is not closed; the reply may have been cut off)", ErrNoJSON)
}

// Decode extracts the first JSON object of a reply into v. Unknown fields are ignored (a model may add commentary
// fields); the caller validates what it needs. Anything after the object other than whitespace and a closing fence
// is ignored too.
func Decode(text string, v any) error {
	raw, err := ExtractJSON(text)
	if err != nil {
		return err
	}
	dec := json.NewDecoder(bytes.NewReader([]byte(raw)))
	if err := dec.Decode(v); err != nil {
		return fmt.Errorf("llm: the reply is not the expected JSON: %w", err)
	}
	return nil
}

// OneOf reports whether s is one of the allowed values.
func OneOf(s string, allowed ...string) bool {
	for _, a := range allowed {
		if s == a {
			return true
		}
	}
	return false
}

// Clip bounds a model-provided string for display: control characters other than newline and tab are dropped,
// invalid UTF-8 is replaced, and the text is cut at max bytes on a character boundary.
func Clip(s string, max int) string {
	s = strings.ToValidUTF8(s, "�")
	s = strings.Map(func(r rune) rune {
		if r == '\n' || r == '\t' || r >= 0x20 && r != 0x7f {
			return r
		}
		return -1
	}, s)
	if max > 0 && len(s) > max {
		cut := max
		for cut > 0 && !utf8RuneStart(s[cut]) {
			cut--
		}
		s = s[:cut] + "…"
	}
	return strings.TrimSpace(s)
}

func utf8RuneStart(b byte) bool { return b&0xC0 != 0x80 }
