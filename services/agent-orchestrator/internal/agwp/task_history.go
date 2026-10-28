package agwp

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"strconv"
)

const taskHistoryPageSize = 200
const maxTaskHistoryRuns = 10000

// ListTaskRuns reads complete task history for retry-budget recovery. A truncated
// history must never be interpreted as a task that has spent no attempts.
func (c *Client) ListTaskRuns(ctx context.Context, repo, agent, task string) ([]Run, error) {
	if task == "" {
		return nil, fmt.Errorf("task history requires a task id")
	}
	caps, err := c.Negotiate(ctx)
	if err != nil {
		return nil, err
	}
	if !caps.Has("tasks.history") {
		return c.legacyTaskRuns(ctx, repo, agent, task)
	}
	return c.pagedTaskRuns(ctx, repo, agent, task)
}

func (c *Client) pagedTaskRuns(ctx context.Context, repo, agent, task string) ([]Run, error) {
	var runs []Run
	cursor := ""
	seen := map[string]bool{}
	seenRuns := map[string]bool{}
	for {
		page, next, err := c.taskRunsPage(ctx, repo, agent, task, cursor)
		if err != nil {
			return nil, err
		}
		if err := validateHistoryPage(len(runs), page, seenRuns); err != nil {
			return nil, err
		}
		runs = append(runs, page...)
		if next == "" {
			return runs, nil
		}
		if seen[next] || len(page) == 0 {
			return nil, fmt.Errorf("task history did not advance to another page")
		}
		seen[next] = true
		cursor = next
	}
}

func validateHistoryPage(total int, page []Run, seen map[string]bool) error {
	if len(page) > taskHistoryPageSize || total+len(page) > maxTaskHistoryRuns {
		return fmt.Errorf("task history exceeds the recovery limit of %d runs", maxTaskHistoryRuns)
	}
	for _, run := range page {
		if run.RunID == "" || seen[run.RunID] {
			return fmt.Errorf("task history contains a missing or repeated run id")
		}
		seen[run.RunID] = true
	}
	return nil
}

func (c *Client) taskRunsPage(ctx context.Context, repo, agent, task, cursor string) ([]Run, string, error) {
	q := url.Values{"agent": {agent}, "task_id": {task}, "limit": {strconv.Itoa(taskHistoryPageSize)}, "page_token": {cursor}}
	var out struct {
		Runs []Run  `json:"runs"`
		Next string `json:"next_page_token"`
	}
	if err := c.repoCall(ctx, "agent.list_runs", http.MethodGet, repo, "/runs", q, nil, &out); err != nil {
		return nil, "", err
	}
	for _, run := range out.Runs {
		if run.TaskID != task || (agent != "" && run.Agent != agent) {
			return nil, "", fmt.Errorf("server returned unrelated task history")
		}
	}
	return out.Runs, out.Next, nil
}

func (c *Client) legacyTaskRuns(ctx context.Context, repo, agent, task string) ([]Run, error) {
	runs, err := c.ListRuns(ctx, repo, agent, false, taskHistoryPageSize)
	if err != nil {
		return nil, err
	}
	if len(runs) >= taskHistoryPageSize {
		return nil, fmt.Errorf("cannot recover retry budget from truncated history; update the server to support tasks.history")
	}
	var matches []Run
	for _, run := range runs {
		if run.TaskID == task {
			matches = append(matches, run)
		}
	}
	return matches, nil
}
