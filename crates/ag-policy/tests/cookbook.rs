//! The policy cookbook (`docs/tutorials/policy-cookbook.md`) through the real parser.
//!
//! Every ```json block of the page must carry a marker, and the marker says what the parser must do with it:
//!
//! * `<!-- policy: NAME -->`: a recipe. `Policy::parse` must accept it, and what it stores must parse back to the
//!   same policy;
//! * `<!-- policy-invalid: NAME: reason -->`: a mistake. `Policy::parse` must refuse it with an error that contains
//!   `reason`;
//! * `<!-- json -->`: JSON that is not a policy (for example the output of a command). Skipped.
//!
//! A block without a marker fails the test, so a recipe cannot be added to the page without being tested, and the
//! page cannot keep a recipe that the parser has started to refuse. The doc-snippet harness (`scripts/doc_snippets.py`)
//! additionally stores the recipes marked *run* in a real repository.
#![allow(missing_docs)]

use std::collections::BTreeSet;

use ag_policy::Policy;

const PAGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/tutorials/policy-cookbook.md");

#[derive(Debug)]
enum Marker {
    Recipe(String),
    Mistake { name: String, reason: String },
    Other,
}

#[derive(Debug)]
struct Block {
    marker: Marker,
    body: String,
    line: usize,
}

fn parse_marker(comment: &str, line: usize) -> Option<Marker> {
    let body = comment.trim().strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    if let Some(name) = body.strip_prefix("policy-invalid:") {
        let (name, reason) = name
            .split_once(':')
            .unwrap_or_else(|| panic!("line {line}: `policy-invalid` needs `NAME: reason`, got `{body}`"));
        return Some(Marker::Mistake { name: name.trim().to_string(), reason: reason.trim().to_string() });
    }
    if let Some(name) = body.strip_prefix("policy:") {
        return Some(Marker::Recipe(name.trim().to_string()));
    }
    (body == "json").then_some(Marker::Other)
}

fn json_blocks(text: &str) -> Vec<Block> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut pending: Option<Marker> = None;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i].trim();
        if l.starts_with("<!--") {
            if let Some(m) = parse_marker(l, i + 1) {
                assert!(pending.is_none(), "line {}: two markers in a row", i + 1);
                pending = Some(m);
            }
            i += 1;
            continue;
        }
        if let Some(lang) = l.strip_prefix("```") {
            let end = (i + 1..lines.len())
                .find(|&j| lines[j].trim() == "```")
                .unwrap_or_else(|| panic!("line {}: unclosed code fence", i + 1));
            if lang.trim() == "json" {
                let marker = pending.take().unwrap_or_else(|| {
                    panic!("line {}: a json block without a marker (policy, policy-invalid, json)", i + 1)
                });
                out.push(Block { marker, body: lines[i + 1..end].join("\n"), line: i + 1 });
            } else {
                assert!(pending.is_none(), "line {}: a policy marker is followed by a ```{lang} block", i + 1);
            }
            i = end + 1;
            continue;
        }
        assert!(
            pending.is_none() || l.is_empty(),
            "line {}: a marker must be followed directly by its code block, found `{l}`",
            i + 1
        );
        i += 1;
    }
    assert!(pending.is_none(), "a marker at the end of the page has no block");
    out
}

#[test]
fn every_recipe_is_accepted_and_every_mistake_is_refused_by_the_real_parser() {
    let text = std::fs::read_to_string(PAGE).unwrap_or_else(|e| panic!("{PAGE}: {e}"));
    let blocks = json_blocks(&text);
    let mut names = BTreeSet::new();
    let (mut recipes, mut mistakes) = (0, 0);
    for b in &blocks {
        match &b.marker {
            Marker::Recipe(name) => {
                assert!(names.insert(name.clone()), "line {}: the name `{name}` is used twice", b.line);
                let policy = Policy::parse(&b.body)
                    .unwrap_or_else(|e| panic!("recipe `{name}` (line {}) is refused by the parser: {e}", b.line));
                let stored = Policy::parse(&policy.to_json()).expect("the canonical form parses");
                assert_eq!(stored, policy, "recipe `{name}` changes when stored and read back");
                recipes += 1;
            }
            Marker::Mistake { name, reason } => {
                assert!(names.insert(name.clone()), "line {}: the name `{name}` is used twice", b.line);
                let err = Policy::parse(&b.body)
                    .expect_err(&format!("the mistake `{name}` (line {}) is accepted by the parser", b.line));
                assert!(
                    err.to_string().contains(reason.as_str()),
                    "mistake `{name}` (line {}) is refused, but with `{err}` and not for `{reason}`",
                    b.line
                );
                mistakes += 1;
            }
            Marker::Other => {}
        }
    }
    // The test must not pass because the page lost its content.
    assert!(recipes >= 8, "only {recipes} recipes found; the cookbook is supposed to have at least eight");
    assert!(mistakes >= 6, "only {mistakes} mistakes found; the cookbook is supposed to show at least six");
}

#[test]
fn the_recipes_are_not_trivially_the_default_policy() {
    // A recipe that parses to `Policy::default()` would pass the test above while demonstrating nothing.
    let text = std::fs::read_to_string(PAGE).unwrap();
    for b in json_blocks(&text) {
        if let Marker::Recipe(name) = &b.marker {
            assert_ne!(Policy::parse(&b.body).unwrap(), Policy::default(), "recipe `{name}` configures nothing");
        }
    }
}
