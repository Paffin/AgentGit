package testutil

import (
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agentkey"
	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/agwp"
)

// CIWorker plays the trusted execution plane against the real server: it
// claims real jobs and submits real signed attestations for the exact tree of
// the job. (The Rust ag-worker does the same in production; here the test
// decides the result of each check.)
type CIWorker struct {
	w      *World
	issuer string
	key    *agentkey.Key
	token  string
}

// PublicKey is the hex public key a policy must trust for this worker's attestations.
func (c *CIWorker) PublicKey() string { return c.key.PublicHex() }

// Claim takes the next queued job; nil when there is none.
func (c *CIWorker) Claim() *agwp.Job {
	c.w.T.Helper()
	code, body := c.w.Srv.Call(c.token, "POST", "/v1/jobs/claim", map[string]any{"worker": "ci-test", "lease_ms": 60000, "wait_ms": 0})
	if code != 200 {
		c.w.T.Fatalf("claim: %d %s", code, body)
	}
	var out struct {
		Job *agwp.Job `json:"job"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		c.w.T.Fatalf("claim: %v: %s", err, body)
	}
	return out.Job
}

// Complete signs and submits the attestation ("pass" or "fail") for the exact tree of the job.
func (c *CIWorker) Complete(job *agwp.Job, result string) {
	c.w.T.Helper()
	v := job.Validation
	if v == nil {
		c.w.T.Fatalf("job %s carries no validation", job.ID)
	}
	tree, err := hex.DecodeString(v.TreeID)
	if err != nil || len(tree) != 32 {
		c.w.T.Fatalf("tree id %q", v.TreeID)
	}
	id := "att_" + job.ID
	issuedAt := time.Now().UnixMilli()
	resultByte := byte(1)
	if result == "fail" {
		resultByte = 2
	}
	var b []byte
	b = canonStr(b, id)
	b = append(b, tree...)
	b = canonStr(b, v.Check)
	b = append(b, resultByte)
	b = canonVarint(b, v.PolicyVersion)
	b = canonStr(b, "env-test")
	b = canonStr(b, c.issuer)
	b = binary.BigEndian.AppendUint64(b, uint64(issuedAt))
	sig, err := c.key.Sign("attestation.v1", b)
	if err != nil {
		c.w.T.Fatal(err)
	}
	att := map[string]any{
		"id": id, "tree_id": v.TreeID, "check": v.Check, "result": result, "policy_version": v.PolicyVersion,
		"environment_digest": "env-test", "issuer": c.issuer, "issued_at_ms": issuedAt, "public_key": c.key.PublicHex(), "signature": sig,
	}
	code, body := c.w.Srv.Call(c.token, "POST", "/v1/repos/"+c.w.Repo+"/jobs/"+job.ID+"/complete",
		map[string]any{"worker": "ci-test", "attestation": att, "result_json": map[string]any{"exit": 0}})
	if code != 200 {
		c.w.T.Fatalf("complete job %s: %d %s", job.ID, code, body)
	}
}

func canonVarint(b []byte, v uint64) []byte {
	for {
		x := byte(v & 0x7f)
		v >>= 7
		if v == 0 {
			return append(b, x)
		}
		b = append(b, x|0x80)
	}
}

func canonStr(b []byte, s string) []byte {
	b = canonVarint(b, uint64(len(s)))
	return append(b, s...)
}
