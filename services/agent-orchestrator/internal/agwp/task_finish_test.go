package agwp

import (
	"context"
	"encoding/json"
	"net/http"
	"testing"
)

func TestConditionalFinishRequiresServerSupport(t *testing.T) {
	writes := 0
	client, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/v1/capabilities" {
			_ = json.NewEncoder(w).Encode(Capabilities{ProtocolVersion: ProtocolVersion})
			return
		}
		writes++
		_ = json.NewEncoder(w).Encode(Task{})
	}))
	_, err := client.FinishTaskAtSnapshot(context.Background(), "a/b", Task{ID: "task", AttemptsStarted: 1}, TaskDone)
	if err == nil || writes != 0 {
		t.Fatalf("unsupported server received completion: writes=%d err=%v", writes, err)
	}
}

func TestConditionalFinishPreservesZeroLimitAndExecutionGeneration(t *testing.T) {
	for _, state := range []string{TaskDone, TaskCancelled} {
		t.Run(state, func(t *testing.T) {
			var sent map[string]any
			client, _, _ := newTestClient(t, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path == "/v1/capabilities" {
					_ = json.NewEncoder(w).Encode(Capabilities{ProtocolVersion: ProtocolVersion, Features: []string{"tasks.finish_preconditions"}})
					return
				}
				if err := json.NewDecoder(r.Body).Decode(&sent); err != nil {
					t.Error(err)
				}
				_ = json.NewEncoder(w).Encode(Task{ID: "task", State: state})
			}))
			_, err := client.FinishTaskAtSnapshot(context.Background(), "a/b", Task{ID: "task", AttemptsStarted: 3}, state)
			if err != nil {
				t.Fatal(err)
			}
			if sent["expected_attempts_started"] != float64(3) {
				t.Fatalf("missing execution generation: %+v", sent)
			}
			limit, present := sent["expected_max_attempts"]
			if state == TaskCancelled && (!present || limit != float64(0)) {
				t.Fatalf("zero policy was omitted: %+v", sent)
			}
			if state == TaskDone && present {
				t.Fatalf("successful execution was tied to a mutable retry policy: %+v", sent)
			}
		})
	}
}
