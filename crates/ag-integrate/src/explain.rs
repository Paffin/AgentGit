//! A refusal in plain language, with the next commands.
//!
//! An integration that does not land says *why* in a stable code (`missing_validation`, `content`, ...). This
//! module turns each code into a short explanation and the exact commands that fix it. Both the CLI's `ag explain`
//! and the hints printed under a refusal use it, so the two cannot disagree.
//!
//! Every code that can occur has an explanation of its own, not a generic one: the tests enumerate the codes of
//! the policy ([`ag_policy::Unmet::CODES`]) and of the merge ([`ag_merge::ConflictKind::CODES`]) and fail for a
//! code without one, so a new code cannot be added and forgotten.

use serde::Serialize;
use serde_json::Value;

use crate::types::{Blocker, ConflictInfo};

/// One command that moves the proposal forward, and what it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fix {
    /// The command, exactly as to be typed (a word in `<angle brackets>` is for the reader to fill in).
    pub command: String,
    /// What it achieves.
    pub note: String,
}

/// Why something stops a proposal, in words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Explanation {
    /// The stable code: a blocker's (`missing_validation`), a conflict's (`content`), or `invalid`.
    pub code: String,
    /// `blocked` (provide something and run it again), `denied` (the policy refuses it as it stands) or
    /// `conflict` (the changes overlap).
    pub kind: &'static str,
    /// One line.
    pub summary: String,
    /// What it means and why the rule exists.
    pub why: String,
    /// What to run, in order.
    pub fixes: Vec<Fix>,
}

/// How commands are spelled: against a local repository, or a proposal that lives on a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    /// `ag integrate P`, `ag proposal review P`.
    Local,
    /// `ag remote integrate P`, `ag remote review P`.
    Remote,
}

/// The codes of integration blockers that do not come from the policy engine.
pub const EXTRA_BLOCKER_CODES: &[&str] = &["lease"];

fn fix(command: impl Into<String>, note: impl Into<String>) -> Fix {
    Fix { command: command.into(), note: note.into() }
}

fn text<'a>(detail: &'a Value, key: &str) -> Option<&'a str> {
    detail.get(key).and_then(Value::as_str)
}

fn number(detail: &Value, key: &str) -> Option<u64> {
    detail.get(key).and_then(Value::as_u64)
}

struct Cmds {
    p: String,
    w: Where,
}

impl Cmds {
    fn integrate(&self) -> String {
        match self.w {
            Where::Local => format!("ag integrate {}", self.p),
            Where::Remote => format!("ag remote integrate {}", self.p),
        }
    }
    fn approve(&self) -> String {
        match self.w {
            Where::Local => format!("ag proposal review {} --approve", self.p),
            Where::Remote => format!("ag remote review {} --approve", self.p),
        }
    }
    fn show(&self) -> String {
        match self.w {
            Where::Local => format!("ag proposal show {}", self.p),
            Where::Remote => format!("ag remote proposal {}", self.p),
        }
    }
    fn close(&self) -> String {
        match self.w {
            Where::Local => format!("ag proposal close {}", self.p),
            Where::Remote => format!("ag remote close {}", self.p),
        }
    }
    /// Publish new work to the same proposal.
    fn update(&self) -> Vec<Fix> {
        match self.w {
            Where::Local => vec![
                fix("ag snapshot -m \"<what you changed>\"", "record the new work"),
                fix(
                    format!("ag proposal update {}", self.p),
                    "point the proposal at it (earlier approvals no longer count)",
                ),
            ],
            Where::Remote => vec![
                fix("ag snapshot -m \"<what you changed>\"", "record the new work"),
                fix("ag push", "upload it and open a new proposal (the old one can be closed)"),
            ],
        }
    }
    fn policy(&self) -> String {
        match self.w {
            Where::Local => "ag policy explain".to_string(),
            Where::Remote => "ag remote policy <stream>".to_string(),
        }
    }
}

/// Explains one blocker of an integration (`code` and `detail` are the fields of [`Blocker`]; `message` is its
/// sentence, used when the code is not one this version knows). `proposal` is the id to put in the commands.
pub fn explain_blocker(code: &str, detail: &Value, message: &str, proposal: &str, at: Where) -> Explanation {
    let c = Cmds { p: proposal.to_string(), w: at };
    let (kind, summary, why, fixes): (&'static str, String, String, Vec<Fix>) = match code {
        "missing_validation" => {
            let check = text(detail, "check").unwrap_or("<check>").to_string();
            let mut fixes = match at {
                Where::Local => vec![fix(
                    format!("ag validate run {proposal} --check {check} --key <issuer-key> -- <command>"),
                    "run the check on the exact files that would land and sign the result with a key the policy trusts",
                )],
                Where::Remote => vec![fix(
                    format!("ag remote prepare {proposal} --validate"),
                    "queue the check on the server's workers, if the policy says how to run it",
                )],
            };
            fixes.push(fix(c.integrate(), "land it once the check has passed (it resumes where it stopped)"));
            (
                "blocked",
                format!("The check '{check}' has not passed on this exact change."),
                "The policy requires a signed passing result of this check for the exact files that would land, from a trusted issuer. A result for other files, an older policy version or an untrusted signer does not count.".into(),
                fixes,
            )
        }
        "failed_validation" => (
            "denied",
            format!("The check '{}' failed on this exact change.", text(detail, "check").unwrap_or("?")),
            "A trusted signer reported a failure for these files, and a failure wins over any pass. The change has to change.".into(),
            {
                let mut f = vec![fix(c.show(), "see the proposal and what was checked")];
                f.extend(c.update());
                f.push(fix(
                    match at {
                        Where::Local => format!("ag validate run {proposal} --check {} --key <issuer-key> -- <command>", text(detail, "check").unwrap_or("<check>")),
                        Where::Remote => format!("ag remote prepare {proposal} --validate"),
                    },
                    "check the new version",
                ));
                f
            },
        ),
        "insufficient_approvals" => {
            let scope = text(detail, "scope").unwrap_or("default");
            let (required, have) = (number(detail, "required").unwrap_or(0), number(detail, "have").unwrap_or(0));
            let who = if scope == "default" {
                "Somebody other than the author has to look at the change and approve it.".to_string()
            } else {
                format!("The change touches '{scope}', which the policy protects: it needs its own approvals.")
            };
            (
                "blocked",
                format!("{have} of {required} required approvals ({}).", if scope == "default" { "all changes".into() } else { format!("for '{scope}'") }),
                format!("{who} An approval counts only for the version it was given on, so a reviewer must approve again after the proposal changes. The author's own approval does not count, and for an agent's change the approvers must be people."),
                vec![
                    fix(c.approve(), "a reviewer other than the author approves the current version"),
                    fix(c.show(), "see who has reviewed which version"),
                    fix(c.integrate(), "land it"),
                ],
            )
        }
        "unresolved_threads" => (
            "blocked",
            format!("{} line discussion(s) remain open.", number(detail, "count").unwrap_or(0)),
            "The policy requires every line discussion to be resolved, including discussions on earlier revisions. Read the replies before resolving; this does not replace approval.".into(),
            vec![
                fix(format!("ag remote discussion list {proposal}"), "read discussions through the configured remote, or use Reviews in the Web UI"),
                fix(format!("ag remote discussion replies {proposal} <thread>"), "read every reply and obtain the latest thread version from the list"),
                fix(format!("ag remote discussion resolve {proposal} <thread> --expected-version <version>"), "resolve at the version you read; requires the thread author, proposal author, reviewer or integrator"),
                fix(c.integrate(), "try landing again once all discussions are resolved"),
            ],
        ),
        "changes_requested" => (
            "blocked",
            format!("{} asked for changes.", text(detail, "reviewer").unwrap_or("A reviewer")),
            "A reviewer who is allowed to approve has asked for changes on the current version; that blocks the change until they approve it.".into(),
            {
                let mut f = vec![fix(c.show(), "read what they asked for")];
                f.extend(c.update());
                f.push(fix(c.approve(), "that reviewer approves the new version"));
                f.push(fix(c.integrate(), "land it"));
                f
            },
        ),
        "missing_provenance" => (
            "blocked",
            "An agent made this change but no signed provenance is attached.".into(),
            "The policy wants to know who produced an agent's change: the agent, the run, the model and the checks it ran, signed with the agent's key.".into(),
            vec![
                fix("ag provenance attach <revision> --agent <name> --model <model>", "attach a signed record to the proposal's head revision (see `ag provenance attach --help`)"),
                fix(c.integrate(), "land it"),
            ],
        ),
        "lease" => {
            let scope = text(detail, "scope").unwrap_or("<scope>");
            let holder = text(detail, "holder").unwrap_or("someone");
            let stale = text(detail, "reason") == Some("stale_or_missing_token");
            if stale {
                (
                    "blocked",
                    format!("You hold the exclusive lease on '{scope}' but did not present its current token."),
                    "An exclusive lease carries a fencing token. Integration must present the lease's current token, so that a holder whose lease expired and was renewed cannot land stale work.".into(),
                    vec![
                        fix("ag lease list", "find the lease and its token"),
                        fix(format!("{} --fencing-token <token>", c.integrate()), "land it, presenting the token"),
                    ],
                )
            } else {
                (
                    "blocked",
                    format!("{holder} holds an exclusive lease on '{scope}'."),
                    "While an exclusive lease on a scope is held, only its holder may land changes that touch it. That keeps two agents from rewriting the same area at once.".into(),
                    vec![
                        fix("ag lease list", "see the lease and when it ends"),
                        fix(c.integrate(), "try again after the lease is released or has expired"),
                        fix("ag lease acquire '<scope>' --exclusive", "or take your own lease on the scope once it is free"),
                    ],
                )
            }
        }
        "actor_not_allowed" => (
            "denied",
            "You are not allowed to land changes into this stream.".into(),
            text(detail, "reason").map(|r| format!("The policy says: {r}.")).unwrap_or_else(|| "The policy limits who may run the integration; agents may only do it inside their autonomy envelope.".into())
                + " Running it again as the same identity gives the same answer.",
            vec![
                fix(c.policy(), "read who may land changes and what an agent may do alone"),
                fix(c.integrate(), "ask a person the policy allows to run this"),
            ],
        ),
        "author_not_allowed" => (
            "denied",
            format!("{} may not propose changes to this stream.", text(detail, "author").unwrap_or("The author")),
            "The policy lists who may author proposals to this stream, and the author is not on the list.".into(),
            vec![
                fix(c.policy(), "see who may propose"),
                fix(c.close(), "close this proposal; an allowed identity can propose the same change"),
            ],
        ),
        "too_many_changed_files" => (
            "denied",
            format!(
                "The change touches {} files; the policy allows {}.",
                number(detail, "changed").map_or("too many".to_string(), |n| n.to_string()),
                number(detail, "max").map_or("fewer".to_string(), |n| format!("at most {n}"))
            ),
            "Large changes are hard to review, so the policy caps how many files one integration may change.".into(),
            vec![
                fix(c.close(), "close this proposal"),
                fix("ag snapshot -m \"<a smaller part>\"", "record a smaller part of the work and propose that"),
                fix(c.policy(), "or ask an administrator to raise `max_changed_files`"),
            ],
        ),
        other => (
            "blocked",
            if message.is_empty() { format!("Blocked: {other}.") } else { message.to_string() },
            "This version of the tool does not know this reason; the server's own sentence is shown.".into(),
            vec![fix(c.show(), "see the proposal"), fix(c.integrate(), "try again")],
        ),
    };
    Explanation { code: code.to_string(), kind, summary, why, fixes }
}

/// Explains a blocker of an integration.
pub fn explain_blocked(b: &Blocker, proposal: &str, at: Where) -> Explanation {
    explain_blocker(&b.code, &b.detail, &b.message, proposal, at)
}

/// Explains one conflicting path of a merge (`kind` is [`ConflictInfo::kind`]).
pub fn explain_conflict_kind(path: &str, kind: &str, message: &str, proposal: &str, at: Where) -> Explanation {
    let c = Cmds { p: proposal.to_string(), w: at };
    let resolve = |first: Fix| -> Vec<Fix> {
        let mut f = vec![first];
        f.push(fix("ag snapshot -m \"resolve the conflict in <path>\"", "record the resolution"));
        match at {
            Where::Local => f.push(fix(format!("ag proposal update {proposal}"), "point the proposal at it")),
            Where::Remote => f.push(fix("ag push", "upload the resolved work")),
        }
        f.push(fix(c.integrate(), "land it"));
        f
    };
    let sync = |note: &str| fix("ag sync", note.to_string());
    let (summary, why, fixes): (String, String, Vec<Fix>) = match kind {
        "content" => (
            format!("Both sides edited the same lines of '{path}'."),
            "AgentGit merges line by line and never guesses between two different edits of the same lines.".into(),
            resolve(sync("apply the clean parts and mark the conflicting lines with <<<<<<< ours / >>>>>>> theirs; edit them to the result you want")),
        ),
        "both_added" => (
            format!("Both sides added '{path}' with different content."),
            "Two new files of the same name have no common ancestor to merge from.".into(),
            resolve(sync("brings in the other version; keep one, or merge them by hand")),
        ),
        "delete_modify" => (
            format!("'{path}' was deleted on one side and edited on the other."),
            "Keeping the edit and honouring the deletion cannot both be done; a person has to choose.".into(),
            resolve(sync("brings in the other side; delete the file or keep your edited version")),
        ),
        "binary" => (
            format!("'{path}' is a binary file changed on both sides."),
            "Binary files cannot be merged line by line.".into(),
            resolve(sync("brings in the other version; put the file you want in its place")),
        ),
        "type_change" => (
            format!("'{path}' changed between file and symlink, or its link target changed, on both sides."),
            "A change of kind cannot be combined with another change to the same path.".into(),
            resolve(sync("brings in the other side; make the path what you want it to be")),
        ),
        "too_large" => (
            format!("'{path}' is too large for a line-by-line merge."),
            "Merging huge files is refused rather than done slowly or wrongly.".into(),
            resolve(sync("brings in the other side; take one version of the file")),
        ),
        "file_directory" => (
            format!("'{path}' would have to be both a file and a directory."),
            "One side made it a file, the other put files beneath it.".into(),
            resolve(sync("brings in the other side; rename one of them")),
        ),
        "rename_rename" => (
            format!("Both sides renamed '{path}' to different names."),
            "A file cannot have two new names.".into(),
            resolve(sync("brings in the other side; keep one of the names")),
        ),
        "rename_delete" => (
            format!("'{path}' was renamed on one side and deleted on the other."),
            "The rename and the deletion contradict each other.".into(),
            resolve(sync("brings in the other side; keep the renamed file or delete it")),
        ),
        "directory_rename_ambiguous" => (
            format!("A new file '{path}' was added in a directory the other side moved apart."),
            "The directory's files went to several places, so there is no single place the new file belongs; nothing is guessed.".into(),
            resolve(sync("brings in the other side; move the new file to the directory you want")),
        ),
        "directory_rename_collision" => (
            format!("A new file '{path}' would be moved by a directory rename onto a path that is already taken."),
            "Following the rename would overwrite or duplicate a file.".into(),
            resolve(sync("brings in the other side; rename the new file")),
        ),
        other => (
            if message.is_empty() { format!("'{path}' conflicts ({other}).") } else { format!("'{path}': {message}") },
            "This version of the tool does not know this kind of conflict.".into(),
            resolve(sync("brings in the other side so you can resolve the files it marks")),
        ),
    };
    Explanation { code: kind.to_string(), kind: "conflict", summary, why, fixes }
}

/// Explains a conflict reported by an integration or a preview.
pub fn explain_conflict(c: &ConflictInfo, proposal: &str, at: Where) -> Explanation {
    explain_conflict_kind(&c.path, &c.kind, &c.message, proposal, at)
}

/// Explains why a proposal cannot be integrated at all (`reason` from an invalid outcome or a preview's problem).
pub fn explain_invalid(reason: &str, proposal: &str, at: Where) -> Explanation {
    let c = Cmds { p: proposal.to_string(), w: at };
    Explanation {
        code: "invalid".into(),
        kind: "denied",
        summary: format!("The proposal cannot be integrated as it stands: {reason}"),
        why: "The request itself is unusable: for example the proposal is already closed or integrated, its history is unrelated to the stream's, or objects are missing.".into(),
        fixes: vec![
            fix(c.show(), "see the proposal's state"),
            fix("ag status", "see what else is open"),
            fix("ag doctor", "check the repository if objects are missing"),
        ],
    }
}

#[cfg(test)]
mod tests {
    use ag_merge::ConflictKind;
    use ag_policy::Unmet;
    use serde_json::json;

    use super::*;

    /// Every variant of the policy's `Unmet`, written as an exhaustive match so a new variant stops this from
    /// compiling until it is listed here (and so given an explanation by the test below).
    fn every_unmet() -> Vec<Unmet> {
        let all = vec![
            Unmet::ActorNotAllowed { reason: "r".into() },
            Unmet::AuthorNotAllowed { author: "user:a".into() },
            Unmet::FailedValidation { check: "tests".into(), attestation: "att".into() },
            Unmet::MissingValidation { check: "tests".into() },
            Unmet::InsufficientApprovals { scope: "default".into(), required: 2, have: 1 },
            Unmet::ChangesRequested { reviewer: "user:b".into() },
            Unmet::UnresolvedThreads { count: 2 },
            Unmet::MissingProvenance,
            Unmet::TooManyChangedFiles { changed: 9, max: 3 },
        ];
        for u in &all {
            match u {
                Unmet::ActorNotAllowed { .. }
                | Unmet::AuthorNotAllowed { .. }
                | Unmet::FailedValidation { .. }
                | Unmet::MissingValidation { .. }
                | Unmet::InsufficientApprovals { .. }
                | Unmet::ChangesRequested { .. }
                | Unmet::UnresolvedThreads { .. }
                | Unmet::MissingProvenance
                | Unmet::TooManyChangedFiles { .. } => {}
            }
        }
        all
    }

    fn known(e: &Explanation, code: &str) {
        assert_eq!(e.code, code);
        assert!(!e.summary.is_empty() && !e.why.is_empty(), "{code}");
        assert!(!e.fixes.is_empty(), "{code} offers no next command");
        assert!(
            !e.why.contains("does not know"),
            "{code} fell through to the generic explanation: add one to explain_blocker/explain_conflict_kind"
        );
    }

    #[test]
    fn every_policy_blocker_code_has_an_explanation_of_its_own() {
        let unmet = every_unmet();
        let listed: Vec<&str> = unmet.iter().map(Unmet::code).collect();
        let mut declared: Vec<&str> = Unmet::CODES.to_vec();
        declared.sort_unstable();
        let mut have = listed.clone();
        have.sort_unstable();
        assert_eq!(have, declared, "Unmet::CODES and the variants disagree");
        for u in &unmet {
            let b = crate::types::Blocker::from_unmet(u);
            for at in [Where::Local, Where::Remote] {
                known(&explain_blocked(&b, "prop_1", at), u.code());
            }
        }
        for code in EXTRA_BLOCKER_CODES {
            let detail = json!({"scope": "src/**", "holder": "agent:x", "reason": "held_by_other"});
            known(&explain_blocker(code, &detail, "", "prop_1", Where::Local), code);
        }
    }

    #[test]
    fn every_merge_conflict_code_has_an_explanation_of_its_own() {
        for code in ConflictKind::CODES {
            for at in [Where::Local, Where::Remote] {
                let e = explain_conflict_kind("src/a.rs", code, "", "prop_1", at);
                known(&e, code);
                assert_eq!(e.kind, "conflict");
            }
        }
    }

    #[test]
    fn an_unknown_code_still_says_something_useful() {
        let e = explain_blocker("brand_new", &Value::Null, "the server says so", "prop_1", Where::Local);
        assert_eq!(e.summary, "the server says so");
        assert!(!e.fixes.is_empty());
        let e = explain_conflict_kind("a", "brand_new", "odd", "prop_1", Where::Local);
        assert!(e.summary.contains("odd") && !e.fixes.is_empty());
    }

    #[test]
    fn commands_carry_the_proposal_and_the_details_of_the_blocker() {
        let e = explain_blocker("missing_validation", &json!({"check": "lint"}), "", "prop_9", Where::Local);
        assert!(e.fixes[0].command.contains("ag validate run prop_9 --check lint"), "{:?}", e.fixes);
        assert!(e.fixes.iter().any(|f| f.command == "ag integrate prop_9"));
        let r = explain_blocker("missing_validation", &json!({"check": "lint"}), "", "prop_9", Where::Remote);
        assert!(r.fixes.iter().any(|f| f.command == "ag remote prepare prop_9 --validate"), "{:?}", r.fixes);
        let a = explain_blocker(
            "insufficient_approvals",
            &json!({"scope": ".github/**", "required": 2, "have": 1}),
            "",
            "prop_9",
            Where::Local,
        );
        assert!(a.summary.contains("1 of 2") && a.summary.contains(".github/**"), "{}", a.summary);
        let stale = explain_blocker(
            "lease",
            &json!({"scope": "docs/**", "holder": "agent:me", "reason": "stale_or_missing_token"}),
            "",
            "prop_9",
            Where::Local,
        );
        assert!(stale.fixes.iter().any(|f| f.command.contains("--fencing-token")), "{:?}", stale.fixes);
        let held = explain_blocker(
            "lease",
            &json!({"scope": "docs/**", "holder": "agent:other", "reason": "held_by_other"}),
            "",
            "prop_9",
            Where::Local,
        );
        assert!(held.summary.contains("agent:other"), "{}", held.summary);
    }
}
