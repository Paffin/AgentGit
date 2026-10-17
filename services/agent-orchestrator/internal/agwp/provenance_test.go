package agwp

import (
	"bytes"
	"encoding/json"
	"strings"
	"testing"
)

func TestCanonicalBytesGolden(t *testing.T) {
	rec := ProvenanceRecord{
		Changeset: "abc", Agent: "agent:bot#run_1", RunID: "run_1", Model: "m",
		Rationale: "say \"hi\"\n", CreatedAtMs: 1700000000000, Inputs: []string{"issue #1"},
		Parameters: map[string]string{"b": "2", "a": "1"},
	}
	want := `{"agent":"agent:bot#run_1","changeset":"abc","checks_run":[],"created_at":1700000000000,` +
		`"inputs":["issue #1"],"model":"m","parameters":{"a":"1","b":"2"},"prompt_digest":null,` +
		`"rationale":"say \"hi\"\n","run_id":"run_1","schema":"agentgit.provenance.v1","task_id":null}`
	if got := string(rec.CanonicalBytes()); got != want {
		t.Fatalf("canonical bytes\n got: %s\nwant: %s", got, want)
	}
}

func TestCanonicalBytesAreDeterministicAndValidJSON(t *testing.T) {
	rec := ProvenanceRecord{
		Changeset: "c", Agent: "agent:a#r", TaskID: "t", PromptDigest: "d", ChecksRun: []string{"go test", "vet"},
		Parameters: map[string]string{"z": "1", "m": "2", "a": "3", "é": "ü"},
	}
	first := rec.CanonicalBytes()
	for i := 0; i < 50; i++ {
		if !bytes.Equal(rec.CanonicalBytes(), first) {
			t.Fatal("canonical bytes vary between calls (map order leaked)")
		}
	}
	var back map[string]any
	if err := json.Unmarshal(first, &back); err != nil {
		t.Fatalf("not valid JSON: %v\n%s", err, first)
	}
	if back["task_id"] != "t" || back["schema"] != ProvenanceSchema {
		t.Fatalf("decoded = %v", back)
	}
}

func TestCanonicalStringEscapingFollowsSerde(t *testing.T) {
	tests := []struct{ in, want string }{
		{"plain", `"plain"`},
		{"a\"b", `"a\"b"`},
		{`a\b`, `"a\\b"`},
		{"line\nbreak\ttab\r", `"line\nbreak\ttab\r"`},
		{"\x01\x1f", `"\u0001\u001f"`},
		{"\b\f", `"\b\f"`},
		{"é世界", `"é世界"`}, // non-ASCII stays raw UTF-8
		{"<>&", `"<>&"`}, // no HTML escaping
		{"  ", "\"  \""}, // not escaped, unlike encoding/json
		{"\x7f", "\"\x7f\""},
	}
	for _, tt := range tests {
		var b bytes.Buffer
		writeString(&b, tt.in)
		if b.String() != tt.want {
			t.Errorf("writeString(%q) = %s, want %s", tt.in, b.String(), tt.want)
		}
	}
}

func TestInvalidUTF8IsNormalisedLikeTheJSONTransport(t *testing.T) {
	rec := ProvenanceRecord{Changeset: "c", Agent: "agent:a", Rationale: "bad\xff\xfebytes"}
	canon := string(rec.CanonicalBytes())
	if !strings.Contains(canon, "bad��bytes") {
		t.Fatalf("canonical = %s", canon)
	}
	// What the server sees after our JSON body is decoded must canonicalise identically.
	wire, err := json.Marshal(rec)
	if err != nil {
		t.Fatal(err)
	}
	var onServer ProvenanceRecord
	if err := json.Unmarshal(wire, &onServer); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(onServer.CanonicalBytes(), rec.CanonicalBytes()) {
		t.Fatalf("server-side canonical form differs:\n%s\n%s", onServer.CanonicalBytes(), rec.CanonicalBytes())
	}
}

func TestWireRoundTripKeepsCanonicalBytes(t *testing.T) {
	rec := ProvenanceRecord{
		Changeset: "c", Agent: "agent:a#r", RunID: "r", Model: "m", Rationale: "why ",
		Parameters: map[string]string{"k": "v"}, Inputs: []string{"i"}, CreatedAtMs: 42,
	}
	wire, _ := json.Marshal(rec)
	var back ProvenanceRecord
	if err := json.Unmarshal(wire, &back); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(back.CanonicalBytes(), rec.CanonicalBytes()) {
		t.Fatal("canonical bytes changed over the wire")
	}
	if strings.Contains(string(wire), "null") {
		t.Fatalf("wire form must not contain null lists: %s", wire)
	}
}

func TestSignedMessageLayout(t *testing.T) {
	m := SignedMessage("provenance.v1", []byte("payload"))
	want := append([]byte("agentgit.sig.v1\x00"), 13)
	want = append(want, "provenance.v1payload"...)
	if !bytes.Equal(m, want) {
		t.Fatalf("message = %q, want %q", m, want)
	}
}
