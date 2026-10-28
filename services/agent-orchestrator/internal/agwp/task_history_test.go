package agwp

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"testing"
)

func TestTaskHistoryFollowsPagesWithExactFilters(t *testing.T) {
	pages := 0
	client, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		if r.URL.Path == "/v1/capabilities" {
			_ = json.NewEncoder(w).Encode(Capabilities{ProtocolVersion: ProtocolVersion, Features: []string{"tasks.history"}})
			return
		}
		q := r.URL.Query()
		if q.Get("task_id") != "task-old" || q.Get("agent") != "bot" || q.Get("limit") != "200" {
			t.Errorf("incorrect history filters: %v", q)
		}
		pages++
		next := "next"
		if pages == 2 {
			if q.Get("page_token") != "next" {
				t.Errorf("continuation missing: %v", q)
			}
			next = ""
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"runs": []Run{{RunID: fmt.Sprint(pages), Agent: "bot", TaskID: "task-old"}}, "next_page_token": next})
	}))
	runs, err := client.ListTaskRuns(context.Background(), "acme/repo", "bot", "task-old")
	if err != nil || len(runs) != 2 || pages != 2 {
		t.Fatalf("runs=%+v pages=%d error=%v", runs, pages, err)
	}
}

func TestTaskHistoryRefusesIncompleteOrUnrelatedData(t *testing.T) {
	for _, scenario := range []string{"legacy-complete", "legacy-truncated", "wrong-task", "repeated-cursor", "repeated-run", "empty-page", "later-page-error"} {
		t.Run(scenario, func(t *testing.T) {
			pages := 0
			client, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.Header().Set("Content-Type", "application/json")
				if r.URL.Path == "/v1/capabilities" {
					features := []string{"tasks.history"}
					if strings.HasPrefix(scenario, "legacy-") {
						features = nil
					}
					_ = json.NewEncoder(w).Encode(Capabilities{ProtocolVersion: ProtocolVersion, Features: features})
					return
				}
				pages++
				rows := []Run{{RunID: fmt.Sprint(pages), Agent: "bot", TaskID: "task-old"}}
				next := ""
				switch scenario {
				case "legacy-complete":
					rows = append(rows, Run{RunID: "other", Agent: "bot", TaskID: "other"})
				case "legacy-truncated":
					rows = make([]Run, taskHistoryPageSize)
				case "wrong-task":
					rows[0].TaskID = "other"
				case "repeated-cursor":
					next = "same"
				case "repeated-run":
					rows[0].RunID = "duplicate"
					if pages == 1 {
						next = "next"
					}
				case "empty-page":
					rows = nil
					next = "next"
				case "later-page-error":
					if pages == 1 {
						next = "next"
					} else {
						w.WriteHeader(http.StatusForbidden)
						return
					}
				}
				_ = json.NewEncoder(w).Encode(map[string]any{"runs": rows, "next_page_token": next})
			}))
			runs, err := client.ListTaskRuns(context.Background(), "acme/repo", "bot", "task-old")
			if scenario == "legacy-complete" {
				if err != nil || len(runs) != 1 {
					t.Fatalf("runs=%+v error=%v", runs, err)
				}
			} else if err == nil || runs != nil {
				t.Fatalf("incomplete history was accepted: runs=%+v error=%v", runs, err)
			}
		})
	}
}
