// Package secret holds a string that must never reach logs, status output or
// error messages by accident. The only way to read it is Reveal.
package secret

import (
	"fmt"
	"log/slog"
	"strings"
)

// Redacted is what every formatter prints instead of the value.
const Redacted = "[redacted]"

// Secret wraps a credential.
type Secret struct{ v string }

// New wraps s.
func New(s string) Secret { return Secret{v: s} }

// Reveal returns the wrapped value. Call it only where the value is actually
// needed (an Authorization header, a child process environment).
func (s Secret) Reveal() string { return s.v }

// Empty reports whether no value is set.
func (s Secret) Empty() bool { return s.v == "" }

// String implements fmt.Stringer.
func (s Secret) String() string { return Redacted }

// GoString implements fmt.GoStringer.
func (s Secret) GoString() string { return Redacted }

// Format implements fmt.Formatter so no verb can print the value.
func (s Secret) Format(f fmt.State, _ rune) { _, _ = f.Write([]byte(Redacted)) }

// LogValue implements slog.LogValuer.
func (s Secret) LogValue() slog.Value { return slog.StringValue(Redacted) }

// MarshalJSON implements json.Marshaler: secrets are never serialized.
func (s Secret) MarshalJSON() ([]byte, error) { return []byte(`"` + Redacted + `"`), nil }

// MarshalText implements encoding.TextMarshaler.
func (s Secret) MarshalText() ([]byte, error) { return []byte(Redacted), nil }

// UnmarshalJSON accepts a JSON string so wire types can carry a Secret.
func (s *Secret) UnmarshalJSON(b []byte) error {
	str := strings.TrimSpace(string(b))
	if str == "null" {
		return nil
	}
	if len(str) < 2 || str[0] != '"' || str[len(str)-1] != '"' {
		return fmt.Errorf("secret: expected a JSON string")
	}
	// Tokens are plain ASCII without escapes; reject anything else rather than
	// guess at unescaping.
	inner := str[1 : len(str)-1]
	if strings.ContainsAny(inner, `\"`) {
		return fmt.Errorf("secret: unexpected escape in token")
	}
	s.v = inner
	return nil
}

// Scrub replaces every occurrence of each non-empty secret in text.
func Scrub(text string, secrets ...Secret) string {
	for _, s := range secrets {
		if len(s.v) >= 8 { // very short strings would mangle ordinary text
			text = strings.ReplaceAll(text, s.v, Redacted)
		}
	}
	return text
}

// PendingSuffixLength finds an unfinished known value at the end of a live snapshot.
// Call after removing complete secrets. The caller withholds these bytes until more output arrives.
func PendingSuffixLength(text, value string) int {
	if value == "" || strings.HasSuffix(text, value) {
		return 0
	}
	limit := min(len(text), len(value)-1)
	if limit == 0 {
		return 0
	}
	pattern := value[:limit]
	prefix := prefixLengths(pattern)
	matched := 0
	for i := len(text) - limit; i < len(text); i++ {
		if matched == len(pattern) {
			matched = prefix[matched-1]
		}
		for matched > 0 && pattern[matched] != text[i] {
			matched = prefix[matched-1]
		}
		if pattern[matched] == text[i] {
			matched++
		}
	}
	return matched
}

// Prefix failure links keep matching linear even for long, repetitive secrets.
func prefixLengths(pattern string) []int {
	prefix := make([]int, len(pattern))
	for i, matched := 1, 0; i < len(pattern); i++ {
		for matched > 0 && pattern[i] != pattern[matched] {
			matched = prefix[matched-1]
		}
		if pattern[i] == pattern[matched] {
			matched++
		}
		prefix[i] = matched
	}
	return prefix
}
