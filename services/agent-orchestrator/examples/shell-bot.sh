#!/bin/sh
# A minimal command executor: it speaks AGWP with curl, as the agent, with the
# scoped credential the orchestrator gave it. It opens a proposal and stops.
# It does not sign provenance, so a stream whose policy requires provenance from
# agents will refuse to integrate its proposal (that is the point of the example).
set -eu
api() { curl -fsS -H "authorization: Bearer $AG_TOKEN" -H "ag-run: $AG_RUN" -H 'content-type: application/json' "$@"; }
base="$AG_SERVER/v1/repos/$AG_REPO"
ws="ws-$AG_RUN"
api -X POST "$base/workspaces" -d "{\"name\":\"$ws\",\"stream\":\"$AG_STREAM\"}" > /dev/null
content=$(printf 'Written by shell-bot for task %s\n' "$AG_TASK" | base64 | tr -d '\n')
head=$(api -X POST "$base/workspaces/$ws/changes" \
  -d "{\"message\":\"Add a note\",\"changes\":[{\"path\":\"notes/shell-bot.md\",\"op\":\"put\",\"content\":\"$content\"}]}" \
  | sed 's/.*"head":"\([^"]*\)".*/\1/')
api -X POST "$base/proposals" \
  -d "{\"stream\":\"$AG_STREAM\",\"head\":\"$head\",\"title\":\"Note from shell-bot\",\"task_id\":\"$AG_TASK\"}" > /dev/null
echo "opened a proposal for changeset $head"
