//! The four independent verdicts of a proposal (Mergeable, Validated, Approved, Integrated) and the blockers
//! behind them, in words.
//!
//! The derivation mirrors the Web UI (`apps/web/src/lib/fourstate.ts` and `blockers.ts`): none of the verdicts
//! implies another, each is computed from `integration.prepare`, the reviews and the proposal itself. The
//! server's own blocker message is always kept; the explanation only adds what it means and what can be done.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// Blocker codes that no amount of waiting fixes ("refused"); everything else is "provide it and look again".
const REFUSED: [&str; 4] = ["failed_validation", "actor_not_allowed", "author_not_allowed", "too_many_changed_files"];

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A blocker explained: `{code, title, hint, severity, message}`.
pub fn explain_blocker(b: &Value) -> Value {
    let code = s(b, "code");
    let d = b.get("detail").cloned().unwrap_or(Value::Null);
    let ds = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let dn = |k: &str| d.get(k).and_then(Value::as_u64);
    let (title, hint) = match code {
        "missing_validation" => (
            if ds("check").is_empty() {
                "A required check has not passed on this exact tree".to_string()
            } else {
                format!("Check “{}” has not passed on this exact tree", ds("check"))
            },
            "A trusted runner must submit a passing, signed attestation for this tree. Use request_validation; a new commit or a moved stream changes the tree, so earlier results do not carry over.".to_string(),
        ),
        "failed_validation" => (
            if ds("check").is_empty() {
                "A required check failed on this exact tree".to_string()
            } else {
                format!("Check “{}” failed on this exact tree", ds("check"))
            },
            "A trusted failure cannot be overridden. Fix the code, update the proposal (workspace_write_files then proposal_update) and validate again.".to_string(),
        ),
        "insufficient_approvals" => {
            let (have, required) = (dn("have"), dn("required"));
            let title = match (have, required) {
                (Some(h), Some(r)) => {
                    let missing = r.saturating_sub(h) as usize;
                    let scope = ds("scope");
                    let where_ = if scope.is_empty() || scope == "default" { String::new() } else { format!(" for {scope}") };
                    format!("Needs {} more {}{where_} ({h} of {r})", missing, if missing == 1 { "approval" } else { "approvals" })
                }
                _ => "Not enough approvals".to_string(),
            };
            (title, "An eligible reviewer other than the author must approve the current head. Approvals of an older head no longer count.".to_string())
        }
        "changes_requested" => (
            "A reviewer asked for changes".to_string(),
            "The change stays blocked until that reviewer approves again, after the requested changes.".to_string(),
        ),
        "missing_provenance" => (
            "This agent-authored change has no signed provenance".to_string(),
            "The agent must attach a signed provenance record (model, rationale, checks run) for this head. Signing needs the agent's private key, which ag-mcp does not hold: ask the agent's operator.".to_string(),
        ),
        "lease" => {
            let (holder, scope) = (ds("holder"), ds("scope"));
            let stale = ds("reason").contains("token");
            (
                format!(
                    "An exclusive lease{} is held{}",
                    if scope.is_empty() { String::new() } else { format!(" on {scope}") },
                    if holder.is_empty() { String::new() } else { format!(" by {holder}") }
                ),
                if stale {
                    "Integrate with the lease holder's current fencing token, or wait until the lease is released or expires.".to_string()
                } else {
                    "Wait for the lease to be released or to expire.".to_string()
                },
            )
        }
        "actor_not_allowed" => (
            "You are not allowed to integrate this change under the policy".to_string(),
            "Ask someone the policy lists as an integrator. Agents can only integrate inside the autonomy envelope.".to_string(),
        ),
        "author_not_allowed" => (
            "The author may not propose to this stream".to_string(),
            "The policy's allowed_authors does not include the author.".to_string(),
        ),
        "too_many_changed_files" => (
            "The change touches more files than the policy allows".to_string(),
            "Split it into smaller proposals.".to_string(),
        ),
        _ => (s(b, "message").to_string(), String::new()),
    };
    json!({
        "code": code,
        "title": title,
        "hint": hint,
        "severity": if REFUSED.contains(&code) { "refused" } else { "provide" },
        "message": s(b, "message"),
    })
}

/// One verdict.
fn verdict(state: &str, value: &str, summary: impl Into<String>) -> Value {
    json!({"state": state, "value": value, "summary": summary.into()})
}

/// Reviews on the proposal's current head, one per reviewer principal (the latest decision wins).
fn review_summary(reviews: &[Value], head: &str) -> (Vec<Value>, Vec<Value>, Vec<Value>) {
    let mut latest: BTreeMap<String, &Value> = BTreeMap::new();
    let mut sorted: Vec<&Value> = reviews.iter().collect();
    sorted.sort_by_key(|r| r.get("id").and_then(Value::as_u64).unwrap_or(0));
    for r in sorted {
        let principal = s(r, "reviewer").split('#').next().unwrap_or("").to_string();
        latest.insert(principal, r);
    }
    let (mut approvals, mut changes, mut stale) = (vec![], vec![], vec![]);
    for r in latest.values() {
        let brief = json!({"reviewer": s(r, "reviewer"), "decision": s(r, "decision"), "head": s(r, "head"), "body": s(r, "body")});
        if s(r, "head") == head {
            match s(r, "decision") {
                "approve" => approvals.push(brief),
                "request_changes" => changes.push(brief),
                _ => {}
            }
        } else if s(r, "decision") == "approve" {
            stale.push(brief);
        }
    }
    (approvals, changes, stale)
}

/// Derives the verdicts and the explained blockers.
///
/// `prepare` is the answer of `integration.prepare` (or `None` if it could not be computed).
pub fn derive(proposal: &Value, prepare: Option<&Value>, reviews: &[Value]) -> Value {
    let state = s(proposal, "state");
    let head = s(proposal, "head");
    let (approvals, changes, stale) = review_summary(reviews, head);
    let reviews_json = json!({"approvals": approvals, "changes_requested": changes, "stale_approvals": stale});

    let integrated = match state {
        "integrated" => verdict("ok", "yes", "The stream was updated atomically with this change."),
        "closed" => verdict("na", "closed", "This proposal was closed without being integrated."),
        _ => verdict("pending", "not yet", "The stream has not been updated with this change."),
    };
    if state == "integrated" {
        let done = |name: &str| {
            verdict("ok", "at integration", format!("{name} requirements were satisfied when the stream was updated."))
        };
        return json!({
            "verdicts": {"mergeable": done("Mergeable"), "validated": done("Validated"), "approved": done("Approved"), "integrated": integrated},
            "blockers": [],
            "reviews": reviews_json,
            "ready_to_integrate": false,
            "next_steps": ["Nothing to do: this proposal is integrated."],
        });
    }
    if state == "closed" {
        let na = verdict("na", "n/a", "The proposal is closed.");
        return json!({
            "verdicts": {"mergeable": na.clone(), "validated": na.clone(), "approved": na, "integrated": integrated},
            "blockers": [],
            "reviews": reviews_json,
            "ready_to_integrate": false,
            "next_steps": ["This proposal is closed. Open a new one with propose if the work should still land."],
        });
    }

    let empty = Vec::new();
    let conflicts = prepare.and_then(|p| p.get("conflicts")).and_then(Value::as_array).unwrap_or(&empty);
    let validations = prepare.and_then(|p| p.get("validations")).and_then(Value::as_array).unwrap_or(&empty);
    let blockers_raw = prepare.and_then(|p| p.get("blockers")).and_then(Value::as_array).unwrap_or(&empty);
    let problem = prepare.map(|p| s(p, "problem")).unwrap_or("");
    let strategy = prepare.map(|p| s(p, "strategy")).unwrap_or("");

    // Mergeable
    let mergeable = if prepare.is_none() {
        verdict("unknown", "unknown", "The candidate tree could not be computed right now.")
    } else if !problem.is_empty() {
        verdict("failed", "problem", problem)
    } else if !conflicts.is_empty() {
        let paths: Vec<&str> = conflicts.iter().map(|c| s(c, "path")).collect();
        verdict(
            "failed",
            &plural(conflicts.len(), "conflict", "conflicts"),
            format!(
                "No single result tree: {}. Bring the workspace up to date with workspace_sync and resolve them.",
                paths.join(", ")
            ),
        )
    } else {
        match strategy {
            "fast_forward" => verdict("ok", "clean", "Fast-forward: the stream head is an ancestor of this change."),
            "merge" => {
                verdict("ok", "clean merge", "The stream moved, and the deterministic three-way merge is clean.")
            }
            "already_integrated" => verdict("ok", "already in", "The stream already contains this change."),
            _ => verdict("unknown", "unknown", "No candidate tree could be computed."),
        }
    };

    // Validated
    let validated = if prepare.is_none() {
        verdict("unknown", "unknown", "Required checks could not be looked up right now.")
    } else if validations.is_empty() {
        verdict("na", "none required", "The stream's policy requires no checks.")
    } else {
        let states: Vec<&str> = validations.iter().map(|v| s(v, "state")).collect();
        let passed = states.iter().filter(|x| **x == "passed").count();
        let total = states.len();
        if states.contains(&"failed") {
            verdict("failed", "failed", "A trusted check failed on this exact tree.")
        } else if passed == total {
            verdict(
                "ok",
                &format!("{passed}/{total} passed"),
                "Every required check has a trusted passing attestation for this exact tree.",
            )
        } else if states.contains(&"pending") {
            verdict("pending", "running", "A validation job is queued or running for this tree.")
        } else {
            let missing: Vec<&str> =
                validations.iter().filter(|v| s(v, "state") != "passed").map(|v| s(v, "check")).collect();
            verdict(
                "blocked",
                &format!("{passed}/{total} passed"),
                format!(
                    "No trusted passing attestation for this tree: {}. Use request_validation.",
                    missing.join(", ")
                ),
            )
        }
    };

    // Approved
    let approval_blocker = blockers_raw.iter().find(|b| s(b, "code") == "insufficient_approvals");
    let changes_blocker = blockers_raw.iter().any(|b| s(b, "code") == "changes_requested");
    let approved = if changes_blocker || (prepare.is_none() && !changes.is_empty()) {
        verdict("failed", "changes requested", "An eligible reviewer asked for changes.")
    } else if let Some(b) = approval_blocker {
        let d = b.get("detail").cloned().unwrap_or(Value::Null);
        let have = d.get("have").and_then(Value::as_u64).unwrap_or(approvals.len() as u64);
        let required = d.get("required").and_then(Value::as_u64).unwrap_or(have + 1);
        verdict(
            "blocked",
            &format!("{have}/{required}"),
            format!("{have} of {required} required approvals of the current head."),
        )
    } else if !approvals.is_empty() {
        verdict(
            "ok",
            &plural(approvals.len(), "approval", "approvals"),
            "The approvals the policy requires are in for the current head.",
        )
    } else if prepare.is_none() {
        verdict("unknown", "unknown", "Approvals could not be counted right now.")
    } else {
        verdict("na", "not required", "The policy requires no approval for this change.")
    };

    let blockers: Vec<Value> = blockers_raw.iter().map(explain_blocker).collect();
    let ready = prepare.is_some() && blockers.is_empty() && mergeable["state"] == "ok";
    let mut next: Vec<String> = Vec::new();
    if mergeable["state"] == "failed" {
        next.push("Resolve the conflicts: workspace_sync (optionally with resolutions), then proposal_update.".into());
    }
    for b in &blockers {
        let hint = s(b, "hint");
        if !hint.is_empty() {
            next.push(format!("{}: {hint}", s(b, "title")));
        }
    }
    if ready {
        next.push("Everything the policy asks for is in place. Landing it is the integrator's step: hand over the proposal id.".into());
    }
    json!({
        "verdicts": {"mergeable": mergeable, "validated": validated, "approved": approved, "integrated": integrated},
        "blockers": blockers,
        "reviews": reviews_json,
        "ready_to_integrate": ready,
        "next_steps": next,
        "strategy": strategy,
        "changed_files": prepare.and_then(|p| p.get("changed_files")).cloned().unwrap_or(json!([])),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(state: &str) -> Value {
        json!({"id": "prop_1", "stream": "main", "head": "h1", "state": state})
    }

    #[test]
    fn four_independent_verdicts_with_blockers_in_words() {
        let prep = json!({
            "strategy": "fast_forward", "conflicts": [], "changed_files": ["a"],
            "validations": [{"check": "ci", "state": "missing"}, {"check": "lint", "state": "passed"}],
            "blockers": [
                {"code": "insufficient_approvals", "message": "approvals (default): 0 of 1 required", "detail": {"have": 0, "required": 1, "scope": "default"}},
                {"code": "actor_not_allowed", "message": "agents may not integrate"},
                {"code": "missing_validation", "message": "m", "detail": {"check": "ci"}}
            ]
        });
        let d = derive(&proposal("open"), Some(&prep), &[]);
        assert_eq!(d["verdicts"]["mergeable"]["state"], "ok");
        assert_eq!(d["verdicts"]["validated"]["state"], "blocked");
        assert_eq!(d["verdicts"]["validated"]["value"], "1/2 passed");
        assert_eq!(d["verdicts"]["approved"]["value"], "0/1");
        assert_eq!(d["verdicts"]["integrated"]["value"], "not yet");
        assert_eq!(d["ready_to_integrate"], false);
        let titles: Vec<&str> =
            d["blockers"].as_array().unwrap().iter().map(|b| b["title"].as_str().unwrap()).collect();
        assert!(titles.contains(&"Needs 1 more approval (0 of 1)"), "{titles:?}");
        assert!(titles.contains(&"Check “ci” has not passed on this exact tree"));
        assert_eq!(d["blockers"][1]["severity"], "refused");
        assert_eq!(d["blockers"][0]["severity"], "provide");
        // the server's wording is kept verbatim
        assert_eq!(d["blockers"][0]["message"], "approvals (default): 0 of 1 required");
    }

    #[test]
    fn a_clean_proposal_is_ready_and_nothing_implies_anything_else() {
        let prep = json!({"strategy": "merge", "conflicts": [], "validations": [], "blockers": []});
        let reviews = vec![json!({"id": 1, "reviewer": "user:bob", "decision": "approve", "head": "h1"})];
        let d = derive(&proposal("open"), Some(&prep), &reviews);
        assert_eq!(d["verdicts"]["mergeable"]["value"], "clean merge");
        assert_eq!(d["verdicts"]["validated"]["state"], "na");
        assert_eq!(d["verdicts"]["approved"]["value"], "1 approval");
        assert_eq!(d["ready_to_integrate"], true);
        assert_eq!(d["verdicts"]["integrated"]["state"], "pending");
    }

    #[test]
    fn conflicts_fail_only_the_mergeable_verdict() {
        let prep = json!({"strategy": "", "conflicts": [{"path": "a.rs", "kind": "content"}], "validations": [], "blockers": []});
        let d = derive(&proposal("open"), Some(&prep), &[]);
        assert_eq!(d["verdicts"]["mergeable"]["state"], "failed");
        assert_eq!(d["verdicts"]["mergeable"]["value"], "1 conflict");
        assert!(d["verdicts"]["mergeable"]["summary"].as_str().unwrap().contains("a.rs"));
        assert_eq!(d["verdicts"]["validated"]["state"], "na");
        assert_eq!(d["ready_to_integrate"], false);
    }

    #[test]
    fn approvals_of_an_older_head_do_not_count_and_the_latest_decision_wins() {
        let reviews = vec![
            json!({"id": 1, "reviewer": "user:bob", "decision": "approve", "head": "old"}),
            json!({"id": 2, "reviewer": "user:carol#run_1", "decision": "approve", "head": "h1"}),
            json!({"id": 3, "reviewer": "user:carol", "decision": "request_changes", "head": "h1"}),
        ];
        let d = derive(&proposal("open"), None, &reviews);
        assert_eq!(d["reviews"]["stale_approvals"].as_array().unwrap().len(), 1);
        assert_eq!(d["reviews"]["approvals"].as_array().unwrap().len(), 0);
        assert_eq!(d["reviews"]["changes_requested"].as_array().unwrap().len(), 1);
        assert_eq!(d["verdicts"]["approved"]["state"], "failed");
        assert_eq!(d["verdicts"]["mergeable"]["state"], "unknown");
    }

    #[test]
    fn finished_proposals() {
        let d = derive(&proposal("integrated"), None, &[]);
        assert_eq!(d["verdicts"]["integrated"]["state"], "ok");
        assert_eq!(d["verdicts"]["approved"]["value"], "at integration");
        let d = derive(&proposal("closed"), None, &[]);
        assert_eq!(d["verdicts"]["integrated"]["value"], "closed");
        assert_eq!(d["verdicts"]["mergeable"]["state"], "na");
    }

    #[test]
    fn unknown_codes_fall_back_to_the_servers_message() {
        let e = explain_blocker(&json!({"code": "brand_new", "message": "something new"}));
        assert_eq!(e["title"], "something new");
        assert_eq!(e["severity"], "provide");
    }
}
