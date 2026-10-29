//! `ag explain`: why a proposal does not land yet, in plain words, and the commands that fix each reason.
//!
//! The wording lives in `ag_integrate::explain`, one explanation per refusal code (a test enumerates the codes), and
//! the hints printed under a refusal by `ag integrate` and `ag remote integrate` come from the same place.

use std::fmt::Write as _;

use ag_integrate::explain::{Explanation, Where, explain_blocked, explain_conflict, explain_invalid};
use anyhow::{Result, bail};
use clap::Args;
use serde_json::json;

use crate::ctx::Ctx;
use crate::exit;

#[derive(Args, Debug)]
pub struct ExplainArgs {
    /// Proposal id (any unique prefix). Default: your newest open proposal.
    pub proposal: Option<String>,
}

/// Where a proposal stands, as `ag explain` reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// Nothing is in the way.
    Ready,
    /// Something can be provided.
    Blocked,
    /// Refused as it stands (conflict, denial, invalid).
    Refused,
}

impl Standing {
    fn word(self) -> &'static str {
        match self {
            Standing::Ready => "ready",
            Standing::Blocked => "blocked",
            Standing::Refused => "refused",
        }
    }
}

/// One line per explanation: the first command to run, so a refusal can say what to do without a page of text.
pub fn hints(explanations: &[Explanation], proposal: &str, at: Where) -> String {
    let mut s = steps(explanations, proposal, at);
    s.push('\n');
    let _ = match at {
        Where::Local => write!(s, "`ag explain {proposal}` says why, step by step."),
        Where::Remote => write!(s, "`ag remote prepare {proposal}` shows what is still missing."),
    };
    s
}

/// The commands alone, one per line (without the closing pointer to a longer explanation).
pub fn steps(explanations: &[Explanation], proposal: &str, at: Where) -> String {
    let again = match at {
        Where::Local => format!("ag integrate {proposal}"),
        Where::Remote => format!("ag remote integrate {proposal}"),
    };
    let mut seen: Vec<&str> = Vec::new();
    let mut s = String::new();
    for e in explanations {
        // The step that provides what is missing, not the closing "land it".
        let Some(f) = e.fixes.iter().find(|f| f.command != again) else { continue };
        if seen.contains(&f.command.as_str()) {
            continue;
        }
        seen.push(&f.command);
        let _ = writeln!(s, "  {}", f.command);
    }
    s.trim_end().to_string()
}

/// Explanations of what a server reported about a proposal (`blockers` and `conflicts` of an outcome or a
/// preparation), for the commands that talk to a server.
pub fn from_wire(
    blockers: &[ag_protocol::v1::Blocker],
    conflicts: &[ag_protocol::v1::Conflict],
    proposal: &str,
) -> Vec<Explanation> {
    let mut out: Vec<Explanation> = conflicts
        .iter()
        .map(|c| {
            let mut explanation =
                ag_integrate::explain::explain_conflict_kind(&c.path, &c.kind, &c.message, proposal, Where::Remote);
            explanation.why.push_str(&crate::cmds::conflicts::wire_sources(c));
            explanation
        })
        .collect();
    out.extend(blockers.iter().map(|b| {
        let detail = serde_json::from_str(&b.detail_json).unwrap_or(serde_json::Value::Null);
        ag_integrate::explain::explain_blocker(&b.code, &detail, &b.message, proposal, Where::Remote)
    }));
    out
}

/// The whole explanation of a set of reasons, for a person.
pub fn render(explanations: &[Explanation]) -> String {
    let mut s = String::new();
    for (i, e) in explanations.iter().enumerate() {
        let _ = writeln!(s, "{}. {}", i + 1, e.summary);
        let _ = writeln!(s, "   Why: {}", e.why);
        let _ = writeln!(s, "   Next:");
        for f in &e.fixes {
            let _ = writeln!(s, "     {}", f.command);
            let _ = writeln!(s, "         {}", f.note);
        }
        s.push('\n');
    }
    s.trim_end().to_string()
}

pub fn explain(ctx: &Ctx, a: &ExplainArgs) -> Result<u8> {
    let id = match &a.proposal {
        Some(p) => p.clone(),
        None => {
            let open = ctx.repo.meta().read(|tx| tx.proposals_list(Some("open"), Ctx::cap(0)))?;
            let mine: Vec<&_> = open.iter().filter(|p| p.author == ctx.me.principal().as_str()).collect();
            let pool: Vec<&_> = if mine.is_empty() { open.iter().collect() } else { mine };
            let pick = pool.into_iter();
            match pick.max_by_key(|p| p.created_at) {
                Some(p) => p.id.clone(),
                None => bail!("there is no open proposal to explain; `ag propose -t TITLE` opens one"),
            }
        }
    };
    let prop = ctx.repo.find_proposal(&id)?;
    let p = &prop.id;
    if prop.state != "open" {
        let word = if prop.state == "integrated" { "already landed" } else { "closed" };
        let body = json!({
            "proposal": p, "state": prop.state, "standing": "refused", "explanations": [], "next": [],
            "summary": format!("{p} is {word}; there is nothing to fix."),
        });
        ctx.out(&body, || format!("{p} \"{}\" is {word}; there is nothing to fix.", prop.title))?;
        return Ok(exit::REFUSED);
    }
    let prep = ctx.repo.prepare(p, &ctx.me, &[])?;
    let mut ex: Vec<Explanation> = Vec::new();
    if let Some(problem) = &prep.preview.problem {
        ex.push(explain_invalid(problem, p, Where::Local));
    }
    for c in &prep.preview.conflicts {
        let mut explanation = explain_conflict(c, p, Where::Local);
        explanation.why.push_str(&crate::cmds::conflicts::local_sources(c));
        ex.push(explanation);
    }
    for b in &prep.blockers {
        ex.push(explain_blocked(b, p, Where::Local));
    }
    let standing = if ex.is_empty() {
        Standing::Ready
    } else if ex.iter().all(|e| e.kind == "blocked") {
        Standing::Blocked
    } else {
        Standing::Refused
    };
    let next: Vec<&str> = ex.iter().filter_map(|e| e.fixes.first()).map(|f| f.command.as_str()).collect();
    let body = json!({
        "proposal": p, "title": prop.title, "stream": prop.stream, "standing": standing.word(),
        "policy_version": prep.policy_version, "explanations": ex, "next": next,
    });
    ctx.out(&body, || {
        let head = format!("Proposal {p} \"{}\" -> {}", prop.title, prop.stream);
        match standing {
            Standing::Ready => {
                format!("{head}: nothing is in the way (policy v{}).\nNext:\n  ag integrate {p}", prep.policy_version)
            }
            _ => format!("{head}: {} (policy v{}).\n\n{}", standing.word(), prep.policy_version, render(&ex)),
        }
    })?;
    Ok(match standing {
        Standing::Ready => exit::OK,
        Standing::Blocked => exit::BLOCKED,
        Standing::Refused => exit::REFUSED,
    })
}

#[cfg(test)]
mod tests {
    use ag_integrate::explain::explain_blocker;

    use super::*;

    #[test]
    fn hints_name_the_step_that_provides_what_is_missing_once() {
        let a = explain_blocker(
            "insufficient_approvals",
            &json!({"scope": "default", "required": 1, "have": 0}),
            "",
            "prop_1",
            Where::Local,
        );
        let b = explain_blocker(
            "insufficient_approvals",
            &json!({"scope": "x/**", "required": 1, "have": 0}),
            "",
            "prop_1",
            Where::Local,
        );
        let h = hints(&[a, b], "prop_1", Where::Local);
        assert_eq!(h.matches("ag proposal review prop_1 --approve").count(), 1, "{h}");
        assert!(h.ends_with("`ag explain prop_1` says why, step by step."), "{h}");
    }
}
