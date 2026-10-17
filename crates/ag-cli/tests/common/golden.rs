//! Shape recording and golden-file comparison for the `--json` tests.
#![allow(dead_code, missing_docs)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::Run;

/// Commands whose output is the data itself, not a JSON document.
pub const RAW: &[&str] = &["cat", "completions", "help", "man", "policy example", "remote cat"];

pub fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name)
}

/// Keys and value types of `v`, nothing else.
pub fn shape(v: &Value) -> Value {
    match v {
        Value::Null => json!("null"),
        Value::Bool(_) => json!("bool"),
        Value::Number(_) => json!("number"),
        Value::String(_) => json!("string"),
        Value::Array(items) => match items.iter().map(shape).reduce(merge) {
            Some(m) => Value::Array(vec![m]),
            None => json!([]),
        },
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), shape(v))).collect()),
    }
}

/// The union of two shapes (an array's elements, or one command observed twice).
pub fn merge(a: Value, b: Value) -> Value {
    match (a, b) {
        (a, b) if a == b => a,
        (Value::Object(mut x), Value::Object(y)) => {
            for (k, v) in y {
                let merged = match x.remove(&k) {
                    Some(old) => merge(old, v),
                    None => v,
                };
                x.insert(k, merged);
            }
            Value::Object(x)
        }
        (Value::Array(x), Value::Array(y)) => match (x.into_iter().next(), y.into_iter().next()) {
            (Some(p), Some(q)) => Value::Array(vec![merge(p, q)]),
            (Some(p), None) | (None, Some(p)) => Value::Array(vec![p]),
            (None, None) => json!([]),
        },
        (Value::String(x), Value::String(y)) => {
            let mut parts: Vec<&str> = x.split('|').chain(y.split('|')).collect();
            parts.sort_unstable();
            parts.dedup();
            Value::String(parts.join("|"))
        }
        // A string-typed leaf against a structure: keep the structure and note the leaf is possible.
        (Value::String(s), other) | (other, Value::String(s)) if s == "null" => other,
        (a, _) => a,
    }
}

/// Records the shapes of commands as they run; the same key may be recorded several times.
#[derive(Default)]
pub struct Recorder {
    pub shapes: BTreeMap<String, Value>,
}

impl Recorder {
    pub fn rec(&mut self, key: &str, run: &Run) -> Value {
        let v = run.json();
        let s = shape(&v);
        let merged = match self.shapes.remove(key) {
            Some(old) => merge(old, s),
            None => s,
        };
        self.shapes.insert(key.to_string(), merged);
        v
    }
}

/// Compares (or, with `AG_UPDATE_GOLDEN`, rewrites) a golden file.
pub fn check_golden(name: &str, shapes: &BTreeMap<String, Value>, skip: &[&str]) {
    let path = golden_path(name);
    let mut doc = Map::new();
    doc.insert("schema_version".into(), json!(1));
    doc.insert("raw".into(), json!(RAW));
    let mut commands = Map::new();
    for (k, v) in shapes {
        commands.insert(k.clone(), v.clone());
    }
    if std::env::var_os("AG_UPDATE_GOLDEN").is_some() {
        doc.insert("commands".into(), Value::Object(commands));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string_pretty(&Value::Object(doc)).unwrap() + "\n").unwrap();
        return;
    }
    let want: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("{}: {e}; generate it with AG_UPDATE_GOLDEN=1 cargo test -p ag-cli --test json_shapes", path.display())
    }))
    .unwrap();
    assert_eq!(want["schema_version"], 1, "golden schema version");
    let want_cmds = want["commands"].as_object().unwrap();
    let mut problems = Vec::new();
    for (k, v) in &commands {
        match want_cmds.get(k) {
            None => problems.push(format!("'{k}': not in the golden file; new shape: {v}")),
            Some(w) if w != v => {
                problems.push(format!("'{k}': the JSON shape changed\n   golden: {w}\n   actual: {v}"))
            }
            Some(_) => {}
        }
    }
    for k in want_cmds.keys() {
        if !commands.contains_key(k) && !skip.iter().any(|s| k.starts_with(s)) {
            problems.push(format!("'{k}': in the golden file but this run produced no output for it"));
        }
    }
    assert!(
        problems.is_empty(),
        "the --json output no longer matches {}:\n{}\n\nIf this is intended: AG_UPDATE_GOLDEN=1 cargo test -p ag-cli --test json_shapes, then python3 scripts/gen_cli_docs.py. Removing or retyping a key is a breaking change (see docs/reference/cli-json.md).",
        path.display(),
        problems.join("\n")
    );
}
