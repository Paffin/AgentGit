package agwp

import (
	"context"
	"net/http"
	"net/url"
)

// Operations in this file are NOT used by any orchestrator component. They
// exist for operators' tooling and for the test suites, which play the parts of
// administrators, writers and integrators. A repository test
// (TestOrchestratorNeverIntegrates) fails if non-test code outside this
// package calls them: the service proposes and schedules, it never lands,
// changes policy or moves a stream.

// CreateRepo creates a repository.
func (c *Client) CreateRepo(ctx context.Context, name string) (*Repo, error) {
	var out Repo
	if err := c.do(ctx, &call{op: "repo.create", method: http.MethodPost, path: "/v1/repos", body: map[string]string{"name": name}, out: &out}); err != nil {
		return nil, err
	}
	return &out, nil
}

// GrantRole binds a role to a subject pattern on a repository pattern.
func (c *Client) GrantRole(ctx context.Context, subject, repo, role string) error {
	body := map[string]string{"subject": subject, "repo": repo, "role": role}
	return c.do(ctx, &call{op: "admin.roles.grant", method: http.MethodPost, path: "/v1/roles", body: body})
}

// SetPolicy stores a new policy version for a stream.
func (c *Client) SetPolicy(ctx context.Context, repo, stream, document string) error {
	body := map[string]string{"stream": stream, "document": document}
	return c.repoCall(ctx, "admin.policy.set", http.MethodPut, repo, "/policy", nil, body, nil)
}

// CreateTask creates a task.
func (c *Client) CreateTask(ctx context.Context, repo, title, description string, scope, dependsOn []string) (*Task, error) {
	var out Task
	body := map[string]any{"title": title, "description": description, "scope": nonNil(scope), "depends_on": nonNil(dependsOn)}
	if err := c.repoCall(ctx, "task.create", http.MethodPost, repo, "/tasks", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// CloseProposal closes a proposal without integrating it.
func (c *Client) CloseProposal(ctx context.Context, repo, id string) (*Proposal, error) {
	var out Proposal
	if err := c.repoCall(ctx, "proposal.close", http.MethodPost, repo, "/proposals/"+url.PathEscape(id)+"/close", nil, struct{}{}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

// Integrate asks the server to land a proposal. The stream policy decides;
// the result is a structured outcome, not an error, when something is missing.
func (c *Client) Integrate(ctx context.Context, repo, proposal string, fencingTokens []uint64) (*IntegrationResult, error) {
	var out IntegrationResult
	body := map[string]any{}
	if len(fencingTokens) > 0 {
		body["fencing_tokens"] = fencingTokens
	}
	if err := c.repoCall(ctx, "integration.integrate", http.MethodPost, repo, "/proposals/"+url.PathEscape(proposal)+"/integrate", nil, body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}
