package agwp

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/Paffin/AgentGit/services/agent-orchestrator/internal/secret"
)

// ---- session -----------------------------------------------------------------------

// Negotiate fetches the server's protocol version, features and limits.
func (c *Client) Negotiate(ctx context.Context) (*Capabilities, error) {
	var out Capabilities
	if err := c.do(ctx, &call{op: "session.negotiate", method: http.MethodGet, path: "/v1/capabilities", out: &out}); err != nil {
		return nil, err
	}
	if out.ProtocolVersion != ProtocolVersion {
		return &out, fmt.Errorf("agwp: server speaks %q, this client speaks %q", out.ProtocolVersion, ProtocolVersion)
	}
	return &out, nil
}

// Whoami returns the identity, roles and scope of the credential in use.
func (c *Client) Whoami(ctx context.Context) (*Principal, error) {
	var out Principal
	if err := c.do(ctx, &call{op: "session.whoami", method: http.MethodGet, path: "/v1/whoami", out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- repositories and streams ------------------------------------------------------

// DescribeRepo returns repository metadata (and proves the credential can read it).
func (c *Client) DescribeRepo(ctx context.Context, repo string) (*Repo, error) {
	var out Repo
	if err := c.repoCall(ctx, "repo.describe", http.MethodGet, repo, "", nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ListStreams lists the repository's streams.
func (c *Client) ListStreams(ctx context.Context, repo string) ([]Stream, error) {
	var out struct {
		Streams []Stream `json:"streams"`
	}
	if err := c.repoCall(ctx, "stream.list", http.MethodGet, repo, "/streams", nil, nil, &out); err != nil {
		return nil, err
	}
	return out.Streams, nil
}

// ReadHead reads the head of one stream.
func (c *Client) ReadHead(ctx context.Context, repo, stream string) (*Stream, error) {
	var out Stream
	q := url.Values{"stream": {stream}}
	if err := c.repoCall(ctx, "stream.head", http.MethodGet, repo, "/streams/head", q, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- agents and runs ---------------------------------------------------------------

// ListAgents lists registered agents.
func (c *Client) ListAgents(ctx context.Context, repo string) ([]Agent, error) {
	var out struct {
		Agents []Agent `json:"agents"`
	}
	if err := c.repoCall(ctx, "agent.list", http.MethodGet, repo, "/agents", nil, nil, &out); err != nil {
		return nil, err
	}
	return out.Agents, nil
}

// RegisterAgent registers an agent with its public key (hex).
func (c *Client) RegisterAgent(ctx context.Context, repo, name, publicKeyHex string) (*Agent, error) {
	var out Agent
	body := map[string]string{"name": name, "public_key": publicKeyHex, "role": "agent"}
	if err := c.repoCall(ctx, "agent.register", http.MethodPost, repo, "/agents", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// StartRun starts a run of an agent, optionally claiming a task. A service
// credential may start a run for any agent; the response carries the identity
// the agent acts as.
func (c *Client) StartRun(ctx context.Context, repo, agent, taskID string) (*Run, error) {
	var out Run
	body := map[string]string{"agent": agent}
	if taskID != "" {
		body["task_id"] = taskID
	}
	if err := c.repoCall(ctx, "agent.start_run", http.MethodPost, repo, "/runs", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Heartbeat records that a run is alive.
func (c *Client) Heartbeat(ctx context.Context, repo, runID string) error {
	return c.repoCall(ctx, "agent.heartbeat", http.MethodPost, repo, "/runs/"+url.PathEscape(runID)+"/heartbeat", nil, struct{}{}, nil)
}

// CompleteRun ends a run with status "completed" or "failed" and releases its leases.
func (c *Client) CompleteRun(ctx context.Context, repo, runID, status, summary string) (*Run, error) {
	var out Run
	body := map[string]string{"status": status, "summary": summary}
	if err := c.repoCall(ctx, "agent.complete", http.MethodPost, repo, "/runs/"+url.PathEscape(runID)+"/complete", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ListRuns lists runs, newest first. agent may be empty.
func (c *Client) ListRuns(ctx context.Context, repo, agent string, activeOnly bool, limit int) ([]Run, error) {
	q := url.Values{}
	if agent != "" {
		q.Set("agent", agent)
	}
	if activeOnly {
		q.Set("active_only", "true")
	}
	if limit > 0 {
		q.Set("limit", strconv.Itoa(limit))
	}
	var out struct {
		Runs []Run `json:"runs"`
	}
	if err := c.repoCall(ctx, "agent.list_runs", http.MethodGet, repo, "/runs", q, nil, &out); err != nil {
		return nil, err
	}
	return out.Runs, nil
}

// ---- tasks -------------------------------------------------------------------------

// ListTasks lists tasks, optionally filtered by state and by dependency readiness.
func (c *Client) ListTasks(ctx context.Context, repo, state string, readyOnly bool) ([]Task, error) {
	q := url.Values{}
	if state != "" {
		q.Set("state", state)
	}
	if readyOnly {
		q.Set("ready_only", "true")
	}
	var out struct {
		Tasks []Task `json:"tasks"`
	}
	if err := c.repoCall(ctx, "task.list", http.MethodGet, repo, "/tasks", q, nil, &out); err != nil {
		return nil, err
	}
	return out.Tasks, nil
}

// GetTask reads one task.
func (c *Client) GetTask(ctx context.Context, repo, id string) (*Task, error) {
	var out Task
	if err := c.repoCall(ctx, "task.get", http.MethodGet, repo, "/tasks/"+url.PathEscape(id), nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// SetTaskAttemptLimit updates execution policy without replacing instructions or history.
func (c *Client) SetTaskAttemptLimit(ctx context.Context, repo, id string, max, expected uint32) (*Task, error) {
	var out Task
	body := map[string]any{"max_attempts": max, "expected_max_attempts": expected}
	if err := c.repoCall(ctx, "task.set_attempt_limit", http.MethodPatch, repo, "/tasks/"+url.PathEscape(id)+"/attempt-limit", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// FinishTask marks a task done or cancelled. The server only lets the task's
// owner (or anyone, for an unowned task) do this.
func (c *Client) FinishTask(ctx context.Context, repo, id, state string) (*Task, error) {
	return c.finishTask(ctx, repo, id, state, nil)
}

// FinishTaskAtSnapshot rejects a stale execution generation or cancellation policy.
func (c *Client) FinishTaskAtSnapshot(ctx context.Context, repo string, task Task, state string) (*Task, error) {
	caps, err := c.Negotiate(ctx)
	if err != nil {
		return nil, err
	}
	if !caps.Has("tasks.finish_preconditions") {
		return nil, fmt.Errorf("server lacks tasks.finish_preconditions; upgrade before automatic task completion")
	}
	return c.finishTask(ctx, repo, task.ID, state, &task)
}

func (c *Client) finishTask(ctx context.Context, repo, id, state string, expected *Task) (*Task, error) {
	var out Task
	body := map[string]any{"state": state}
	if expected != nil {
		body["expected_attempts_started"] = expected.AttemptsStarted
		if state == TaskCancelled {
			body["expected_max_attempts"] = expected.MaxAttempts
		}
	}
	if err := c.repoCall(ctx, "task.finish", http.MethodPost, repo, "/tasks/"+url.PathEscape(id)+"/finish", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- leases ------------------------------------------------------------------------

// AcquireLease takes a soft or exclusive lease on a path scope.
func (c *Client) AcquireLease(ctx context.Context, repo, scope string, exclusive bool, ttl time.Duration, taskID string) (*AcquireLeaseResponse, error) {
	var out AcquireLeaseResponse
	body := map[string]any{"scope": scope, "exclusive": exclusive, "ttl_ms": ttl.Milliseconds()}
	if taskID != "" {
		body["task_id"] = taskID
	}
	if err := c.repoCall(ctx, "lease.acquire", http.MethodPost, repo, "/leases", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ExtendLease prolongs a lease; its fencing token does not change.
func (c *Client) ExtendLease(ctx context.Context, repo, id string, ttl time.Duration) (*Lease, error) {
	var out Lease
	if err := c.repoCall(ctx, "lease.extend", http.MethodPost, repo, "/leases/"+url.PathEscape(id)+"/extend", nil, map[string]any{"ttl_ms": ttl.Milliseconds()}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ReleaseLease releases a lease held by the caller.
func (c *Client) ReleaseLease(ctx context.Context, repo, id string) error {
	return c.repoCall(ctx, "lease.release", http.MethodDelete, repo, "/leases/"+url.PathEscape(id), nil, nil, nil)
}

// ListLeases lists leases (only active ones when activeOnly).
func (c *Client) ListLeases(ctx context.Context, repo string, activeOnly bool) ([]Lease, error) {
	q := url.Values{}
	if activeOnly {
		q.Set("active_only", "true")
	}
	var out struct {
		Leases []Lease `json:"leases"`
	}
	if err := c.repoCall(ctx, "lease.list", http.MethodGet, repo, "/leases", q, nil, &out); err != nil {
		return nil, err
	}
	return out.Leases, nil
}

// Predict returns the server's path-level overlap prediction for a stream.
func (c *Client) Predict(ctx context.Context, repo, stream string) ([]Overlap, error) {
	var out struct {
		Overlaps []Overlap `json:"overlaps"`
	}
	if err := c.repoCall(ctx, "agent.predict", http.MethodGet, repo, "/predict", url.Values{"stream": {stream}}, nil, &out); err != nil {
		return nil, err
	}
	return out.Overlaps, nil
}

// ReapReport is what a reap did (or, for a dry run, would do).
type ReapReport struct {
	// Runs are the runs ended as "abandoned".
	Runs []Run `json:"runs"`
	// ReleasedLeases are the ids of the leases released.
	ReleasedLeases []string `json:"released_leases"`
	// RevokedCredentials counts the credentials that were bound to the reaped runs.
	RevokedCredentials int  `json:"revoked_credentials"`
	DryRun             bool `json:"dry_run"`
}

// ReapRuns ends the runs of a repository that have not sent a heartbeat for staleAfter (zero: the server's
// configured default), releases the leases of ended runs and revokes the credentials bound to the reaped runs.
// It is an operator's call (capability agent.operate, held by the delegator role) and is audited under the
// caller's identity.
func (c *Client) ReapRuns(ctx context.Context, repo string, staleAfter time.Duration, dryRun bool) (*ReapReport, error) {
	var out ReapReport
	body := map[string]any{"stale_after_ms": uint64(staleAfter / time.Millisecond), "dry_run": dryRun}
	if err := c.repoCall(ctx, "agent.reap_runs", http.MethodPost, repo, "/runs/reap", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- credentials -------------------------------------------------------------------

// MintRequest asks for a scoped, expiring credential for subject.
type MintRequest struct {
	Subject     string          `json:"subject"`
	Description string          `json:"description,omitempty"`
	TTLSecs     uint32          `json:"ttl_secs"`
	Scope       CredentialScope `json:"scope"`
}

// Minted is a new credential: metadata plus the secret, which is shown once.
type Minted struct {
	Credential Credential    `json:"credential"`
	Token      secret.Secret `json:"token"`
}

// MintCredential mints a credential. The server refuses scopes wider than the
// caller's own, and a child never outlives its parent.
func (c *Client) MintCredential(ctx context.Context, req MintRequest) (*Minted, error) {
	var out Minted
	if err := c.do(ctx, &call{op: "admin.credentials.mint", method: http.MethodPost, path: "/v1/credentials", body: req, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// RevokeCredential revokes a credential immediately.
func (c *Client) RevokeCredential(ctx context.Context, id string) error {
	return c.do(ctx, &call{op: "admin.credentials.revoke", method: http.MethodDelete, path: "/v1/credentials/" + url.PathEscape(id)})
}

// ---- virtual workspaces ------------------------------------------------------------

// CreateWorkspace creates a virtual workspace at the head of a stream.
func (c *Client) CreateWorkspace(ctx context.Context, repo, name, stream string) (*Workspace, error) {
	var out Workspace
	if err := c.repoCall(ctx, "workspace.create", http.MethodPost, repo, "/workspaces", nil, map[string]string{"name": name, "stream": stream}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// GetWorkspace reads a workspace.
func (c *Client) GetWorkspace(ctx context.Context, repo, name string) (*Workspace, error) {
	var out Workspace
	if err := c.repoCall(ctx, "workspace.get", http.MethodGet, repo, "/workspaces/"+url.PathEscape(name), nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ModifyWorkspace applies a batch of file changes as one new changeset.
// expectedHead, when set, makes the call fail with failed_precondition if the
// workspace moved.
func (c *Client) ModifyWorkspace(ctx context.Context, repo, name, message, expectedHead string, changes []FileChange) (*Workspace, error) {
	var out Workspace
	body := map[string]any{"message": message, "changes": changes}
	if expectedHead != "" {
		body["expected_head"] = expectedHead
	}
	if err := c.repoCall(ctx, "workspace.modify", http.MethodPost, repo, "/workspaces/"+url.PathEscape(name)+"/changes", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// SyncResult is the outcome of bringing the stream head into a workspace.
type SyncResult struct {
	Status    string     `json:"status"` // up_to_date | fast_forward | merged | conflicted
	Head      string     `json:"head"`
	Conflicts []Conflict `json:"conflicts"`
}

// SyncWorkspace merges the stream head into the workspace. Conflicts are
// reported and change nothing until every conflicting path is passed in
// resolutions.
func (c *Client) SyncWorkspace(ctx context.Context, repo, name string, resolutions []FileChange) (*SyncResult, error) {
	var out SyncResult
	body := map[string]any{}
	if len(resolutions) > 0 {
		body["resolutions"] = resolutions
	}
	if err := c.repoCall(ctx, "workspace.sync", http.MethodPost, repo, "/workspaces/"+url.PathEscape(name)+"/sync", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// DeleteWorkspace removes a workspace.
func (c *Client) DeleteWorkspace(ctx context.Context, repo, name string) error {
	return c.repoCall(ctx, "workspace.delete", http.MethodDelete, repo, "/workspaces/"+url.PathEscape(name), nil, nil, nil)
}

// ---- proposals and reviews ---------------------------------------------------------

// ProposalInput describes a proposal to open.
type ProposalInput struct {
	Stream      string `json:"stream"`
	Head        string `json:"head"`
	Title       string `json:"title"`
	Description string `json:"description,omitempty"`
	TaskID      string `json:"task_id,omitempty"`
}

// CreateProposal opens a proposal (publishing the head and its ancestors first).
func (c *Client) CreateProposal(ctx context.Context, repo string, in ProposalInput) (*Proposal, error) {
	var out Proposal
	if err := c.repoCall(ctx, "proposal.create", http.MethodPost, repo, "/proposals", nil, in, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ProposalFilter narrows ListProposals.
type ProposalFilter struct {
	State  string
	Stream string
	Author string
}

// ListProposals lists all proposals matching the filter (following pagination).
func (c *Client) ListProposals(ctx context.Context, repo string, f ProposalFilter) ([]Proposal, error) {
	var all []Proposal
	token := ""
	for page := 0; page < 1000; page++ {
		q := url.Values{"page_size": {"1000"}}
		if f.State != "" {
			q.Set("state", f.State)
		}
		if f.Stream != "" {
			q.Set("stream", f.Stream)
		}
		if f.Author != "" {
			q.Set("author", f.Author)
		}
		if token != "" {
			q.Set("page_token", token)
		}
		var out struct {
			Proposals     []Proposal `json:"proposals"`
			NextPageToken string     `json:"next_page_token"`
		}
		if err := c.repoCall(ctx, "proposal.list", http.MethodGet, repo, "/proposals", q, nil, &out); err != nil {
			return nil, err
		}
		all = append(all, out.Proposals...)
		if out.NextPageToken == "" {
			return all, nil
		}
		token = out.NextPageToken
	}
	return all, fmt.Errorf("agwp: proposal listing did not terminate")
}

// GetProposal reads a proposal with its reviews.
func (c *Client) GetProposal(ctx context.Context, repo, id string) (*ProposalDetail, error) {
	var out ProposalDetail
	if err := c.repoCall(ctx, "proposal.get", http.MethodGet, repo, "/proposals/"+url.PathEscape(id), nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Review decisions. The orchestrator only ever posts DecisionComment.
const (
	DecisionComment = "comment"
	DecisionApprove = "approve"
	DecisionChanges = "request_changes"
)

// ReviewProposal records a review of the proposal's current head.
func (c *Client) ReviewProposal(ctx context.Context, repo, id, decision, body string) (*Review, error) {
	var out Review
	if err := c.repoCall(ctx, "proposal.review", http.MethodPost, repo, "/proposals/"+url.PathEscape(id)+"/reviews", nil, map[string]string{"decision": decision, "body": body}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ---- integration (read-only side) --------------------------------------------------

// Prepare asks what integrating a proposal would do now and what is missing.
// With enqueue, the server also queues the validation jobs the policy knows
// how to run (this needs the integrate.validate capability). Prepare never
// changes a stream.
func (c *Client) Prepare(ctx context.Context, repo, proposal string, enqueue bool) (*Preparation, error) {
	var out Preparation
	body := map[string]any{"enqueue_validations": enqueue}
	if err := c.repoCall(ctx, "integration.prepare", http.MethodPost, repo, "/proposals/"+url.PathEscape(proposal)+"/prepare", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ListJobs lists execution-plane jobs of a repository, optionally by state.
func (c *Client) ListJobs(ctx context.Context, repo, state string, limit int) ([]Job, error) {
	q := url.Values{}
	if state != "" {
		q.Set("state", state)
	}
	if limit > 0 {
		q.Set("limit", strconv.Itoa(limit))
	}
	var out struct {
		Jobs []Job `json:"jobs"`
	}
	if err := c.repoCall(ctx, "jobs.list", http.MethodGet, repo, "/jobs", q, nil, &out); err != nil {
		return nil, err
	}
	return out.Jobs, nil
}

// ListQueue lists the integration queue of a stream.
func (c *Client) ListQueue(ctx context.Context, repo, stream string) ([]QueueEntry, error) {
	var out struct {
		Entries []QueueEntry `json:"entries"`
	}
	if err := c.repoCall(ctx, "queue.list", http.MethodGet, repo, "/queue", url.Values{"stream": {stream}}, nil, &out); err != nil {
		return nil, err
	}
	return out.Entries, nil
}

// ---- tree and files ----------------------------------------------------------------

// GetTree returns the full manifest of a revision, following pagination.
func (c *Client) GetTree(ctx context.Context, repo, rev string) (*Tree, error) {
	var tree Tree
	token := ""
	for page := 0; page < 10000; page++ {
		q := url.Values{"rev": {rev}, "page_size": {"1000"}}
		if token != "" {
			q.Set("page_token", token)
		}
		var out struct {
			TreeID        string      `json:"tree_id"`
			Entries       []TreeEntry `json:"entries"`
			NextPageToken string      `json:"next_page_token"`
		}
		if err := c.repoCall(ctx, "tree.get", http.MethodGet, repo, "/tree", q, nil, &out); err != nil {
			return nil, err
		}
		tree.TreeID = out.TreeID
		tree.Entries = append(tree.Entries, out.Entries...)
		if out.NextPageToken == "" {
			return &tree, nil
		}
		token = out.NextPageToken
	}
	return nil, fmt.Errorf("agwp: tree listing did not terminate")
}

// GetFile downloads one file at a revision (a changeset id or a stream name).
// Files larger than maxBytes fail instead of being buffered.
func (c *Client) GetFile(ctx context.Context, repo, rev, path string, maxBytes int64) ([]byte, error) {
	p, err := repoPath(repo)
	if err != nil {
		return nil, err
	}
	segs := strings.Split(strings.Trim(path, "/"), "/")
	for i, s := range segs {
		segs[i] = url.PathEscape(s)
	}
	var data []byte
	err = c.do(ctx, &call{
		op: "tree.file", method: http.MethodGet, path: p + "/files/" + strings.Join(segs, "/"),
		query: url.Values{"rev": {rev}}, raw: &data, rawMax: maxBytes,
	})
	return data, err
}

// FileDiff is the change to one file between two revisions.
type FileDiff struct {
	Path      string `json:"path"`
	Status    string `json:"status"` // added | modified | deleted | renamed
	Binary    bool   `json:"binary"`
	Additions int    `json:"additions"`
	Deletions int    `json:"deletions"`
	Patch     string `json:"patch"`
}

// DiffResult is the answer of tree.diff.
type DiffResult struct {
	Files     []FileDiff `json:"files"`
	Truncated bool       `json:"truncated"`
}

// Diff returns per-file unified diffs between two revisions (a stream name or
// a changeset id). contextLines is the number of context lines around hunks.
func (c *Client) Diff(ctx context.Context, repo, from, to string, contextLines int) (*DiffResult, error) {
	var out DiffResult
	body := map[string]any{"from": from, "to": to, "context": contextLines, "paths": []string{}}
	if err := c.repoCall(ctx, "tree.diff", http.MethodPost, repo, "/diff", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// MergeBases returns the merge bases of two revisions (usually one).
func (c *Client) MergeBases(ctx context.Context, repo, a, b string) ([]string, error) {
	var out struct {
		Bases []string `json:"bases"`
	}
	if err := c.repoCall(ctx, "history.merge_base", http.MethodGet, repo, "/history/merge-base", url.Values{"a": {a}, "b": {b}}, nil, &out); err != nil {
		return nil, err
	}
	return out.Bases, nil
}

// ResolveRev resolves a revision expression (a stream name, a changeset id) to a changeset id.
func (c *Client) ResolveRev(ctx context.Context, repo, rev string) (string, error) {
	var out struct {
		ChangesetID string `json:"changeset_id"`
	}
	if err := c.repoCall(ctx, "history.resolve", http.MethodGet, repo, "/history/resolve", url.Values{"rev": {rev}}, nil, &out); err != nil {
		return "", err
	}
	return out.ChangesetID, nil
}

// ---- provenance --------------------------------------------------------------------

// SubmitProvenance stores a record the agent signed with its own key. The
// caller must be authenticated as that agent.
func (c *Client) SubmitProvenance(ctx context.Context, repo string, rec ProvenanceRecord, signatureHex string) (*ProvenanceEntry, error) {
	var out ProvenanceEntry
	body := map[string]any{"record": rec, "signature": signatureHex}
	if err := c.repoCall(ctx, "provenance.submit", http.MethodPost, repo, "/provenance", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ListProvenance lists the provenance records of a changeset.
func (c *Client) ListProvenance(ctx context.Context, repo, changeset string) ([]ProvenanceEntry, error) {
	var out struct {
		Entries []ProvenanceEntry `json:"entries"`
	}
	if err := c.repoCall(ctx, "provenance.list", http.MethodGet, repo, "/provenance", url.Values{"changeset": {changeset}}, nil, &out); err != nil {
		return nil, err
	}
	return out.Entries, nil
}

// ---- events ------------------------------------------------------------------------

// ReplayEvents returns events with seq > afterSeq (oldest first) and the newest
// sequence number in the log. topics are dotted-prefix filters.
func (c *Client) ReplayEvents(ctx context.Context, repo string, afterSeq uint64, limit int, topics []string) ([]Event, uint64, error) {
	q := url.Values{"after_seq": {strconv.FormatUint(afterSeq, 10)}}
	if limit > 0 {
		q.Set("limit", strconv.Itoa(limit))
	}
	if len(topics) > 0 {
		q.Set("topics", strings.Join(topics, ","))
	}
	var out struct {
		Events    []Event `json:"events"`
		LatestSeq uint64  `json:"latest_seq"`
	}
	if err := c.repoCall(ctx, "events.replay", http.MethodGet, repo, "/events/replay", q, nil, &out); err != nil {
		return nil, 0, err
	}
	return out.Events, out.LatestSeq, nil
}

// TaskScheduling reads the launch gate; older servers without this additive operation remain usable.
func (c *Client) TaskScheduling(ctx context.Context, repo string) (*TaskScheduling, error) {
	var out TaskScheduling
	err := c.repoCall(ctx, "task.scheduling.get", http.MethodGet, repo, "/tasks/scheduling", nil, nil, &out)
	if IsNotFound(err) {
		return &out, nil
	}
	if err != nil {
		return nil, err
	}
	return &out, nil
}

// SetTaskScheduling changes the launch gate at an inspected revision.
func (c *Client) SetTaskScheduling(ctx context.Context, repo string, paused bool, reason string, version uint64) (*TaskScheduling, error) {
	var out TaskScheduling
	body := map[string]any{"paused": paused, "reason": reason, "expected_version": version}
	if err := c.repoCall(ctx, "task.scheduling.set", http.MethodPut, repo, "/tasks/scheduling", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// AssignTask selects a registered agent while a task is still queued.
func (c *Client) AssignTask(ctx context.Context, repo, id, agent, expectedOwner string) (*Task, error) {
	var out Task
	body := map[string]any{"agent": agent, "expected_owner": expectedOwner}
	if err := c.repoCall(ctx, "task.assign", http.MethodPut, repo, "/tasks/"+url.PathEscape(id)+"/assignee", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// ReportExecutor renews a bounded connectivity report without sending commands or secrets.
func (c *Client) ReportExecutor(ctx context.Context, repo, agent, name string, slots, available, ttl uint32) error {
	var out ExecutorPresence
	return c.repoCall(ctx, "agent.executor.report", http.MethodPut, repo, "/agents/"+url.PathEscape(agent)+"/executor", nil,
		map[string]any{"name": name, "slots": slots, "available": available, "ttl_secs": ttl}, &out)
}

// SaveRunOutput sends one explicitly enabled, redacted output snapshot as the operator.
func (c *Client) SaveRunOutput(ctx context.Context, repo, run string, output RunOutput) error {
	var out RunOutput
	return c.repoCall(ctx, "agent.output.save", http.MethodPut, repo, "/runs/"+url.PathEscape(run)+"/output", nil, map[string]any{"output": output}, &out)
}

// GetRunOutput reads the last saved snapshot under the run's normal read permission.
func (c *Client) GetRunOutput(ctx context.Context, repo, run string) (*RunOutput, error) {
	var out RunOutput
	if err := c.repoCall(ctx, "agent.output.get", http.MethodGet, repo, "/runs/"+url.PathEscape(run)+"/output", nil, nil, &out); err != nil {
		return nil, err
	}
	return &out, nil
}
