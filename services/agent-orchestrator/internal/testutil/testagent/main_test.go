package main

import "testing"

func TestParseScript(t *testing.T) {
	desc := "Please do the thing.\nag-script: {\"files\": {\"a.txt\": \"A\\n\"}, \"delete\": [\"old.txt\"], \"title\": \"T\", \"message\": \"M\"}\nthanks"
	ts, ok := parseScript(desc)
	if !ok || ts.Files["a.txt"] != "A\n" || len(ts.Delete) != 1 || ts.Title != "T" || ts.Message != "M" {
		t.Fatalf("ts = %+v ok=%v", ts, ok)
	}
	for _, bad := range []string{"", "no marker here", "ag-script: not json", "ag-script: {\"unknown\": 1}"} {
		if _, ok := parseScript(bad); ok {
			t.Errorf("%q must not parse", bad)
		}
	}
}
