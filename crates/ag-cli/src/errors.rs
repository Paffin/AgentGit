//! Error presentation: one line that says what went wrong, one line that says what to do next.
//!
//! Errors come from many layers (engine, storage, client, argument checks), most of them written to be
//! precise rather than helpful. [`hint_for`] adds the missing "what now" from the message text. A hint is
//! advice, never a decision: nothing here changes an exit code.

use serde_json::json;

use crate::ui::{self, Tone};

/// Flattens an error chain into one message, dropping links whose text the previous link already contains
/// (several engine errors repeat their source in their own message).
pub fn message_of(e: &anyhow::Error) -> String {
    let mut out = String::new();
    for cause in e.chain() {
        let s = cause.to_string();
        if out.is_empty() {
            out = s;
        } else if !out.contains(&s) {
            out.push_str(": ");
            out.push_str(&s);
        }
    }
    out
}

/// What to do about `msg`, if we know.
pub fn hint_for(msg: &str) -> Option<&'static str> {
    // Ordered: the first matching needle wins, so the specific ones come first. Messages that already say
    // what to do (they contain "run `ag", "see `ag", "use `") are left alone.
    if msg.contains("`ag ")
        && (msg.contains("run `ag")
            || msg.contains("see `ag")
            || msg.contains("use `ag")
            || msg.contains("add one with `ag"))
    {
        return None;
    }
    const TABLE: &[(&str, &str)] = &[
        (
            "has no changesets yet",
            "land a first change: `ag snapshot -m MSG`, `ag propose -t TITLE`, `ag integrate PROPOSAL`",
        ),
        ("no snapshots yet", "record the working directory first: `ag snapshot -m MSG`"),
        ("not found: proposal", "list proposals with `ag proposal list` (any unique id prefix of 6+ characters works)"),
        ("not found: operation", "list operations with `ag op list`"),
        ("not found: no operation with idempotency key", "list operations with `ag op list`"),
        ("not found: lease", "list leases with `ag lease list --all`"),
        ("not found: task", "list tasks with `ag task list`"),
        ("not found: workspace", "list workspaces with `ag workspace list`"),
        ("no stream '", "list streams with `ag stream list`"),
        ("not found: stream", "list streams with `ag stream list`"),
        ("no pin named", "list pins with `ag pins`"),
        (
            "cannot resolve '",
            "a revision is a stream, workspace (`@name`), proposal id or changeset prefix; try `ag stream list` or `ag log`; see `ag help revisions`",
        ),
        ("is ambiguous", "give more characters of the id"),
        ("uncommitted change", "record them with `ag snapshot -m MSG` (or discard them) and run the command again"),
        (
            "a conflicted sync is pending",
            "resolve the conflict markers in the listed files, then `ag snapshot -m \"resolve\"`",
        ),
        ("unresolved conflict markers", "resolve the markers, or use `ag snapshot --force` if they are intentional"),
        ("is already an AgentGit repository", "use `ag status` to see where you are, or pick another directory"),
        ("already exists: agent", "list agents with `ag agent list`"),
        ("is not empty", "choose an empty or new directory"),
        ("is readable by other users", "restrict the file: `chmod 600 <file>`"),
        (
            "database is locked",
            "another `ag` process holds the repository's write lock; wait for it and run the command again",
        ),
        ("schema version", "this repository was written by a newer `ag`; upgrade `ag` (there is no downgrade)"),
        ("invalid policy document", "see `ag policy example` for a valid document and `ag help policy` for the fields"),
        ("bad tree id", "tree ids are 64 hex characters; `ag preview PROPOSAL` prints the candidate tree"),
        (
            "nothing to validate: the proposal is already integrated",
            "validation applies to proposals that are still open: `ag proposal list --state open`",
        ),
        ("git rev-parse", "the path must be a Git repository (check with `git -C PATH status`)"),
        ("cannot run git", "install `git`; only `ag git import|export` needs it"),
        ("does not exist at", "list the files at that revision with `ag ls REV`"),
        ("expected REV:PATH", "for the workspace head use `@:PATH`, for a stream `main:PATH`; see `ag help revisions`"),
        ("--lines takes START,END", "for example `-L 10,20`"),
        ("choose one of --approve", "for example `ag proposal review PROPOSAL --approve -m \"looks good\"`"),
        (
            "Connection refused",
            "is the server running and reachable at that URL? check with `ag remote list` and `ag doctor`",
        ),
        (
            "connection refused",
            "is the server running and reachable at that URL? check with `ag remote list` and `ag doctor`",
        ),
        (
            "Unauthenticated",
            "the credential was rejected (expired, revoked or for another server); mint a new one and check with `ag doctor`",
        ),
        (
            "unauthenticated",
            "the credential was rejected (expired, revoked or for another server); mint a new one and check with `ag doctor`",
        ),
        (
            "PermissionDenied",
            "your credential lacks the role or scope for this; `ag remote whoami` shows what it grants",
        ),
        (
            "permission denied",
            "your credential lacks the role or scope for this; `ag remote whoami` shows what it grants",
        ),
        (
            "no credential for remote",
            "export AG_TOKEN (or AG_TOKEN_<REMOTE>), or store one with `--token-stdin`; `ag doctor` checks every remote",
        ),
        (
            "invalid identity",
            "identities look like `user:alice`, `agent:bot#run_…` or `service:ci`; see `ag help identities`",
        ),
    ];
    TABLE.iter().find(|(needle, _)| msg.contains(needle)).map(|(_, hint)| *hint)
}

/// Formats an error for standard error. Text mode: `error: …` and an optional `hint: …` line. JSON mode:
/// one line, `{"error":{"message":…,"hint":…,"exit_code":N}}`.
pub fn render(e: &anyhow::Error, json: bool, exit_code: u8) -> String {
    let msg = message_of(e);
    let hint = hint_for(&msg);
    if json {
        return json!({"error": {"message": msg, "hint": hint, "exit_code": exit_code}}).to_string();
    }
    let mut s = format!("{} {msg}", ui::tone(Tone::Bad, "error:"));
    if let Some(h) = hint {
        s.push_str(&format!("\n{} {h}", ui::tone(Tone::Hint, "hint:")));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    #[test]
    fn repeated_causes_are_collapsed() {
        let inner = anyhow!("unknown field `bogus` at line 1 column 8");
        let outer = inner.context("invalid policy document: unknown field `bogus` at line 1 column 8");
        let m = message_of(&outer);
        assert_eq!(m.matches("unknown field").count(), 1, "{m}");
        let chain = anyhow!("disk full").context("cannot write the index");
        assert_eq!(message_of(&chain), "cannot write the index: disk full");
    }

    #[test]
    fn hints_name_a_next_step_and_do_not_double_up() {
        assert!(hint_for("not found: proposal 'foo'").unwrap().contains("ag proposal list"));
        assert!(hint_for("no stream 'x'").unwrap().contains("ag stream list"));
        assert!(
            hint_for("not an AgentGit repository (or any parent of /x); run `ag init`").is_none(),
            "already says what to do"
        );
        assert!(hint_for("something nobody foresaw").is_none());
    }

    #[test]
    fn json_errors_are_one_parsable_line() {
        let e = anyhow!("not found: proposal 'foo'");
        let s = render(&e, true, 1);
        assert!(!s.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["error"]["exit_code"], 1);
        assert!(v["error"]["hint"].as_str().unwrap().contains("ag proposal list"));
    }
}
