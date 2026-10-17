package agwp

import (
	"encoding/json"
	"time"
)

// The types below mirror the JSON messages of docs/protocol/openapi.json that
// the orchestrator reads or writes. Field names are the protocol's snake_case
// names; unknown fields are ignored so newer servers keep working.

// Capabilities is the answer of GET /v1/capabilities.
type Capabilities struct {
	ProtocolVersion string   `json:"protocol_version"`
	ServerVersion   string   `json:"server_version"`
	Features        []string `json:"features"`
	AuthMethods     []string `json:"auth_methods"`
	Limits          Limits   `json:"limits"`
}

// Limits are the server's advertised limits.
type Limits struct {
	MaxObjectBytes              uint64 `json:"max_object_bytes"`
	MaxRequestBytes             uint64 `json:"max_request_bytes"`
	MaxBatch                    uint32 `json:"max_batch"`
	MaxPageSize                 uint32 `json:"max_page_size"`
	MaxChangesetOps             uint32 `json:"max_changeset_ops"`
	MaxChangesetsPerPublication uint32 `json:"max_changesets_per_publication"`
}

// Has reports whether the server advertises a feature.
func (c Capabilities) Has(feature string) bool {
	for _, f := range c.Features {
		if f == feature {
			return true
		}
	}
	return false
}

// CredentialScope narrows a credential. Empty lists mean "unrestricted".
type CredentialScope struct {
	Repos      []string `json:"repos"`
	Streams    []string `json:"streams"`
	Operations []string `json:"operations"`
}

// Principal is the answer of GET /v1/whoami.
type Principal struct {
	Identity     string          `json:"identity"`
	Kind         string          `json:"kind"`
	Method       string          `json:"method"`
	CredentialID string          `json:"credential_id"`
	ExpiresAtMs  int64           `json:"expires_at_ms"`
	Roles        []string        `json:"roles"`
	Scope        CredentialScope `json:"scope"`
}

// ExpiresAt returns the credential expiry (zero when it never expires).
func (p Principal) ExpiresAt() time.Time {
	if p.ExpiresAtMs <= 0 {
		return time.Time{}
	}
	return time.UnixMilli(p.ExpiresAtMs)
}

// Credential describes a minted credential (never includes the secret).
type Credential struct {
	ID          string          `json:"id"`
	Subject     string          `json:"subject"`
	Kind        string          `json:"kind"`
	Description string          `json:"description"`
	Scope       CredentialScope `json:"scope"`
	CreatedAtMs int64           `json:"created_at_ms"`
	ExpiresAtMs int64           `json:"expires_at_ms"`
	RevokedAtMs int64           `json:"revoked_at_ms"`
	CreatedBy   string          `json:"created_by"`
}

// Repo is a repository description.
type Repo struct {
	Name          string `json:"name"`
	Namespace     string `json:"namespace"`
	DefaultStream string `json:"default_stream"`
	Description   string `json:"description"`
	CreatedBy     string `json:"created_by"`
	StreamCount   uint32 `json:"stream_count"`
}

// Stream is a stream and its head.
type Stream struct {
	Name          string `json:"name"`
	Head          string `json:"head"`
	Version       uint64 `json:"version"`
	PolicyVersion uint64 `json:"policy_version"`
}

// Agent is a registered agent.
type Agent struct {
	Name      string `json:"name"`
	Identity  string `json:"identity"`
	PublicKey string `json:"public_key"`
	Role      string `json:"role"`
	Disabled  bool   `json:"disabled"`
}

// Run is one execution of an agent.
type Run struct {
	RunID           string `json:"run_id"`
	Agent           string `json:"agent"`
	Identity        string `json:"identity"`
	TaskID          string `json:"task_id"`
	Status          string `json:"status"`
	Summary         string `json:"summary"`
	StartedAtMs     int64  `json:"started_at_ms"`
	FinishedAtMs    int64  `json:"finished_at_ms"`
	LastHeartbeatMs int64  `json:"last_heartbeat_ms"`
}

// Task states.
const (
	TaskOpen       = "open"
	TaskInProgress = "in_progress"
	TaskDone       = "done"
	TaskCancelled  = "cancelled"
)

// Task is a unit of work with dependencies and a declared scope.
type Task struct {
	ID          string   `json:"id"`
	Title       string   `json:"title"`
	Description string   `json:"description"`
	State       string   `json:"state"`
	Owner       string   `json:"owner"`
	DependsOn   []string `json:"depends_on"`
	Scope       []string `json:"scope"`
	Ready       bool     `json:"ready"`
	CreatedBy   string   `json:"created_by"`
	CreatedAtMs int64    `json:"created_at_ms"`
	UpdatedAtMs int64    `json:"updated_at_ms"`
}

// Lease is a soft or exclusive claim on a path scope.
type Lease struct {
	ID           string `json:"id"`
	Scope        string `json:"scope"`
	Holder       string `json:"holder"`
	Exclusive    bool   `json:"exclusive"`
	FencingToken uint64 `json:"fencing_token"`
	AcquiredAtMs int64  `json:"acquired_at_ms"`
	ExpiresAtMs  int64  `json:"expires_at_ms"`
	ReleasedAtMs int64  `json:"released_at_ms"`
	TaskID       string `json:"task_id"`
}

// Active reports whether the lease is neither released nor expired at now.
func (l Lease) Active(now time.Time) bool {
	return l.ReleasedAtMs == 0 && l.ExpiresAtMs > now.UnixMilli()
}

// AcquireLeaseResponse carries the granted lease and overlapping ones.
type AcquireLeaseResponse struct {
	Lease       Lease   `json:"lease"`
	Overlapping []Lease `json:"overlapping"`
}

// Workspace is a server-side virtual workspace.
type Workspace struct {
	Name   string `json:"name"`
	Stream string `json:"stream"`
	Base   string `json:"base"`
	Head   string `json:"head"`
	Owner  string `json:"owner"`
}

// FileChange is one entry of a workspace modification.
type FileChange struct {
	Path       string `json:"path"`
	Op         string `json:"op"` // "put" or "delete"
	Content    []byte `json:"content,omitempty"`
	Executable bool   `json:"executable,omitempty"`
}

// Proposal states.
const (
	ProposalOpen       = "open"
	ProposalIntegrated = "integrated"
	ProposalClosed     = "closed"
)

// Proposal is a request to move a stream to a new head.
type Proposal struct {
	ID           string `json:"id"`
	Stream       string `json:"stream"`
	Head         string `json:"head"`
	Title        string `json:"title"`
	Description  string `json:"description"`
	Author       string `json:"author"`
	State        string `json:"state"`
	TaskID       string `json:"task_id"`
	CreatedAtMs  int64  `json:"created_at_ms"`
	UpdatedAtMs  int64  `json:"updated_at_ms"`
	IntegratedAs string `json:"integrated_as"`
}

// Review is a review of a proposal head.
type Review struct {
	ID          int64  `json:"id"`
	Proposal    string `json:"proposal"`
	Reviewer    string `json:"reviewer"`
	Decision    string `json:"decision"`
	Body        string `json:"body"`
	Head        string `json:"head"`
	TimestampMs int64  `json:"timestamp_ms"`
}

// ProposalDetail is a proposal with its reviews.
type ProposalDetail struct {
	Proposal Proposal `json:"proposal"`
	Reviews  []Review `json:"reviews"`
}

// Conflict is one conflicting path of a merge.
type Conflict struct {
	Path string `json:"path"`
	Kind string `json:"kind"`
}

// Blocker is a stable-coded reason an integration cannot proceed.
type Blocker struct {
	Code    string          `json:"code"`
	Message string          `json:"message"`
	Detail  json.RawMessage `json:"detail"`
}

// Blocker codes the orchestrator interprets (docs/guides/policy.md#outcome-codes).
const (
	BlockerMissingValidation    = "missing_validation"
	BlockerFailedValidation     = "failed_validation"
	BlockerInsufficientApproval = "insufficient_approvals"
	BlockerChangesRequested     = "changes_requested"
	BlockerMissingProvenance    = "missing_provenance"
	BlockerLease                = "lease"
	BlockerActorNotAllowed      = "actor_not_allowed"
)

// ValidationStatus is where one required check stands.
type ValidationStatus struct {
	Check         string `json:"check"`
	State         string `json:"state"` // passed | failed | missing | pending
	AttestationID string `json:"attestation_id"`
	JobID         string `json:"job_id"`
}

// Job states.
const (
	JobQueued    = "queued"
	JobClaimed   = "claimed"
	JobSucceeded = "succeeded"
	JobFailed    = "failed"
)

// Job is a queued unit of work for the trusted execution plane. The
// orchestrator only ever reads jobs: claiming, running and attesting them is
// the job of runner-role workers whose signing keys the policy trusts.
type Job struct {
	ID             string          `json:"id"`
	Kind           string          `json:"kind"`
	State          string          `json:"state"`
	Repo           string          `json:"repo"`
	Attempts       uint32          `json:"attempts"`
	ClaimedBy      string          `json:"claimed_by"`
	ClaimedUntilMs int64           `json:"claimed_until_ms"`
	CreatedAtMs    int64           `json:"created_at_ms"`
	UpdatedAtMs    int64           `json:"updated_at_ms"`
	Error          string          `json:"error"`
	Validation     *ValidationTask `json:"validation"`
}

// ValidationTask says which check to run on which exact tree.
type ValidationTask struct {
	Check                     string   `json:"check"`
	Proposal                  string   `json:"proposal"`
	Stream                    string   `json:"stream"`
	TreeID                    string   `json:"tree_id"`
	PolicyVersion             uint64   `json:"policy_version"`
	RequiredEnvironmentDigest string   `json:"required_environment_digest"`
	Run                       *RunSpec `json:"run"`
}

// RunSpec is how the policy says a check should be run.
type RunSpec struct {
	Argv        []string `json:"argv"`
	TimeoutSecs uint32   `json:"timeout_secs"`
	MemoryMB    uint32   `json:"memory_mb"`
	Network     bool     `json:"network"`
}

// Preparation is the answer of integration.prepare.
type Preparation struct {
	Proposal      string             `json:"proposal"`
	Strategy      string             `json:"strategy"`
	CandidateTree string             `json:"candidate_tree"`
	Conflicts     []Conflict         `json:"conflicts"`
	Base          string             `json:"base"`
	ChangedFiles  []string           `json:"changed_files"`
	Problem       string             `json:"problem"`
	Algorithm     string             `json:"algorithm"`
	PolicyVersion uint64             `json:"policy_version"`
	Blockers      []Blocker          `json:"blockers"`
	Validations   []ValidationStatus `json:"validations"`
	Jobs          []Job              `json:"jobs"`
}

// HasBlocker reports whether a blocker with the code is present.
func (p Preparation) HasBlocker(code string) bool {
	for _, b := range p.Blockers {
		if b.Code == code {
			return true
		}
	}
	return false
}

// IntegrationOutcome is the structured result of integrate.
type IntegrationOutcome struct {
	Status        string     `json:"status"`
	NewHead       string     `json:"new_head"`
	PreviousHead  string     `json:"previous_head"`
	Version       uint64     `json:"version"`
	Strategy      string     `json:"strategy"`
	TreeID        string     `json:"tree_id"`
	ChangedFiles  uint32     `json:"changed_files"`
	Blockers      []Blocker  `json:"blockers"`
	Conflicts     []Conflict `json:"conflicts"`
	Reason        string     `json:"reason"`
	PolicyVersion uint64     `json:"policy_version"`
}

// IntegrationResult wraps an outcome with the operation id.
type IntegrationResult struct {
	OperationID string             `json:"operation_id"`
	State       string             `json:"state"`
	Outcome     IntegrationOutcome `json:"outcome"`
}

// Overlap is one predicted overlap of work.
type Overlap struct {
	A     string   `json:"a"`
	B     string   `json:"b"`
	Kind  string   `json:"kind"`
	Paths []string `json:"paths"`
}

// TreeEntry is one file of a tree manifest.
type TreeEntry struct {
	Path       string `json:"path"`
	BlobID     string `json:"blob_id"`
	Size       uint64 `json:"size"`
	FileType   string `json:"file_type"`
	Executable bool   `json:"executable"`
}

// Tree is a whole-tree manifest.
type Tree struct {
	TreeID  string      `json:"tree_id"`
	Entries []TreeEntry `json:"entries"`
}

// Changeset is a (published) changeset.
type Changeset struct {
	ID          string   `json:"id"`
	Parents     []string `json:"parents"`
	Author      string   `json:"author"`
	Message     string   `json:"message"`
	TimestampMs int64    `json:"timestamp_ms"`
}

// Event is one entry of the repository's audit-backed event feed.
type Event struct {
	Seq         uint64       `json:"seq"`
	Topic       string       `json:"topic"`
	Repo        string       `json:"repo"`
	TimestampMs int64        `json:"timestamp_ms"`
	Payload     EventPayload `json:"payload"`
}

// EventPayload is the common envelope of an event's payload.
type EventPayload struct {
	Actor   string          `json:"actor"`
	Target  string          `json:"target"`
	Details json.RawMessage `json:"details"`
}

// Detail decodes one string-valued key of the details object.
func (e Event) Detail(key string) string {
	var m map[string]json.RawMessage
	if json.Unmarshal(e.Payload.Details, &m) != nil {
		return ""
	}
	var s string
	if json.Unmarshal(m[key], &s) != nil {
		return ""
	}
	return s
}

// QueueEntry is an entry of the integration queue.
type QueueEntry struct {
	ID         int64  `json:"id"`
	Stream     string `json:"stream"`
	Proposal   string `json:"proposal"`
	State      string `json:"state"`
	Outcome    string `json:"outcome"`
	EnqueuedBy string `json:"enqueued_by"`
}

// ProvenanceRecord is what an agent claims about a change it produced. The
// signature covers CanonicalBytes (see provenance.go).
type ProvenanceRecord struct {
	Changeset    string            `json:"changeset"`
	Agent        string            `json:"agent"`
	RunID        string            `json:"run_id"`
	TaskID       string            `json:"task_id"`
	Model        string            `json:"model"`
	PromptDigest string            `json:"prompt_digest"`
	Parameters   map[string]string `json:"parameters"`
	ChecksRun    []string          `json:"checks_run"`
	Inputs       []string          `json:"inputs"`
	Rationale    string            `json:"rationale"`
	CreatedAtMs  int64             `json:"created_at_ms"`
}

// ProvenanceEntry is a stored provenance record.
type ProvenanceEntry struct {
	ID        int64            `json:"id"`
	Record    ProvenanceRecord `json:"record"`
	PublicKey string           `json:"public_key"`
	Signature string           `json:"signature"`
	Verified  bool             `json:"verified"`
}

// nonNil returns s, or an empty slice: the server's decoder rejects JSON null
// for repeated fields, so a nil slice must be sent as [].
func nonNil(s []string) []string {
	if s == nil {
		return []string{}
	}
	return s
}

// MarshalJSON renders nil lists as [] (see nonNil).
func (s CredentialScope) MarshalJSON() ([]byte, error) {
	type plain CredentialScope
	s.Repos, s.Streams, s.Operations = nonNil(s.Repos), nonNil(s.Streams), nonNil(s.Operations)
	return json.Marshal(plain(s))
}

// MarshalJSON renders nil lists and maps as [] and {} (see nonNil).
func (r ProvenanceRecord) MarshalJSON() ([]byte, error) {
	type plain ProvenanceRecord
	r.ChecksRun, r.Inputs = nonNil(r.ChecksRun), nonNil(r.Inputs)
	if r.Parameters == nil {
		r.Parameters = map[string]string{}
	}
	return json.Marshal(plain(r))
}
