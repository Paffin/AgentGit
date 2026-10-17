package agwp

import (
	"bytes"
	"sort"
	"strconv"
	"unicode/utf8"
)

// ProvenanceSchema is the schema tag of provenance records.
const ProvenanceSchema = "agentgit.provenance.v1"

// ProvenanceDomain is the signature domain of provenance records.
const ProvenanceDomain = "provenance.v1"

// SigPrefix starts every signed message (docs/protocol/agwp-v1.md#signing-formats).
const SigPrefix = "agentgit.sig.v1\x00"

// SignedMessage builds the bytes an Ed25519 signature covers:
// prefix ‖ len(domain):u8 ‖ domain ‖ payload.
func SignedMessage(domain string, payload []byte) []byte {
	m := make([]byte, 0, len(SigPrefix)+1+len(domain)+len(payload))
	m = append(m, SigPrefix...)
	m = append(m, byte(len(domain)))
	m = append(m, domain...)
	return append(m, payload...)
}

// Normalize applies to the record what JSON transport would: invalid UTF-8 in
// strings becomes U+FFFD, byte by byte (encoding/json's rule). Signing a
// normalized record guarantees that the server, which sees the transported
// form, computes the same canonical bytes.
func (r ProvenanceRecord) Normalize() ProvenanceRecord {
	r.Changeset, r.Agent, r.RunID, r.TaskID = fixUTF8(r.Changeset), fixUTF8(r.Agent), fixUTF8(r.RunID), fixUTF8(r.TaskID)
	r.Model, r.PromptDigest, r.Rationale = fixUTF8(r.Model), fixUTF8(r.PromptDigest), fixUTF8(r.Rationale)
	if r.Parameters != nil {
		p := make(map[string]string, len(r.Parameters))
		for k, v := range r.Parameters {
			p[fixUTF8(k)] = fixUTF8(v)
		}
		r.Parameters = p
	}
	r.ChecksRun = fixAll(r.ChecksRun)
	r.Inputs = fixAll(r.Inputs)
	return r
}

func fixAll(in []string) []string {
	if in == nil {
		return nil
	}
	out := make([]string, len(in))
	for i, s := range in {
		out[i] = fixUTF8(s)
	}
	return out
}

func fixUTF8(s string) string {
	if utf8.ValidString(s) {
		return s
	}
	var b bytes.Buffer
	for i := 0; i < len(s); {
		r, size := utf8.DecodeRuneInString(s[i:])
		if r == utf8.RuneError && size == 1 {
			b.WriteString("�")
		} else {
			b.WriteString(s[i : i+size])
		}
		i += size
	}
	return b.String()
}

// CanonicalBytes renders the record the way the server does before verifying a
// signature: compact JSON, keys sorted alphabetically at every level, absent
// optional fields as null, timestamps in Unix milliseconds, and strings
// escaped like serde_json (only quotes, backslashes and control characters
// are escaped; everything else is raw UTF-8).
func (r ProvenanceRecord) CanonicalBytes() []byte {
	r = r.Normalize()
	var b bytes.Buffer
	b.WriteByte('{')
	field := func(name string, write func()) {
		if b.Len() > 1 {
			b.WriteByte(',')
		}
		writeString(&b, name)
		b.WriteByte(':')
		write()
	}
	optional := func(s string) func() {
		return func() {
			if s == "" {
				b.WriteString("null")
			} else {
				writeString(&b, s)
			}
		}
	}
	list := func(items []string) func() {
		return func() {
			b.WriteByte('[')
			for i, s := range items {
				if i > 0 {
					b.WriteByte(',')
				}
				writeString(&b, s)
			}
			b.WriteByte(']')
		}
	}
	// Keys in alphabetical order, as serde_json's ordered map emits them.
	field("agent", func() { writeString(&b, r.Agent) })
	field("changeset", func() { writeString(&b, r.Changeset) })
	field("checks_run", list(r.ChecksRun))
	field("created_at", func() { b.WriteString(strconv.FormatInt(r.CreatedAtMs, 10)) })
	field("inputs", list(r.Inputs))
	field("model", optional(r.Model))
	field("parameters", func() {
		keys := make([]string, 0, len(r.Parameters))
		for k := range r.Parameters {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		b.WriteByte('{')
		for i, k := range keys {
			if i > 0 {
				b.WriteByte(',')
			}
			writeString(&b, k)
			b.WriteByte(':')
			writeString(&b, r.Parameters[k])
		}
		b.WriteByte('}')
	})
	field("prompt_digest", optional(r.PromptDigest))
	field("rationale", optional(r.Rationale))
	field("run_id", optional(r.RunID))
	field("schema", func() { writeString(&b, ProvenanceSchema) })
	field("task_id", optional(r.TaskID))
	b.WriteByte('}')
	return b.Bytes()
}

const hexDigits = "0123456789abcdef"

// writeString writes s as a JSON string with serde_json's escaping rules.
func writeString(b *bytes.Buffer, s string) {
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch c {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\b':
			b.WriteString(`\b`)
		case '\f':
			b.WriteString(`\f`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if c < 0x20 {
				b.WriteString(`\u00`)
				b.WriteByte(hexDigits[c>>4])
				b.WriteByte(hexDigits[c&0xf])
			} else {
				b.WriteByte(c)
			}
		}
	}
	b.WriteByte('"')
}
