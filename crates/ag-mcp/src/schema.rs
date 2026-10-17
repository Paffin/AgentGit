//! A small JSON Schema (2020-12) validator for the tool input and output schemas this crate publishes.
//!
//! It implements exactly the keywords the schemas in [`crate::tools`] use; [`check_supported`] rejects a schema
//! that uses anything else, so a tool author cannot silently publish a constraint that is not enforced.
//! `format`, `description`, `title`, `default` and `examples` are annotations and never fail validation.

use serde_json::Value;

const KEYWORDS: &[&str] = &[
    "$schema",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "enum",
    "const",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "items",
    "minItems",
    "maxItems",
    "anyOf",
    "description",
    "title",
    "default",
    "examples",
    "format",
];

/// Errors if `schema` uses a keyword this validator does not enforce.
pub fn check_supported(schema: &Value) -> Result<(), String> {
    fn walk(s: &Value, path: &str) -> Result<(), String> {
        let Some(o) = s.as_object() else {
            return if s.is_boolean() { Ok(()) } else { Err(format!("{path}: a schema must be an object or boolean")) };
        };
        for (k, v) in o {
            if !KEYWORDS.contains(&k.as_str()) {
                return Err(format!("{path}: unsupported keyword `{k}`"));
            }
            match k.as_str() {
                "properties" => {
                    for (name, sub) in v.as_object().ok_or(format!("{path}/properties must be an object"))? {
                        walk(sub, &format!("{path}/properties/{name}"))?;
                    }
                }
                "items" | "additionalProperties" => walk(v, &format!("{path}/{k}"))?,
                "anyOf" => {
                    for (i, sub) in v.as_array().ok_or(format!("{path}/anyOf must be an array"))?.iter().enumerate() {
                        walk(sub, &format!("{path}/anyOf/{i}"))?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    walk(schema, "#")
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn type_matches(expected: &str, v: &Value) -> bool {
    match expected {
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0 && v.is_number()),
        other => type_name(v) == other,
    }
}

/// Validates `instance` against `schema`. Returns every violation found (at most 20), each prefixed with the
/// JSON path of the offending value, e.g. `changes[1].path: expected string, got number`.
pub fn validate(schema: &Value, instance: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(schema, instance, "", &mut errors);
    errors.truncate(20);
    errors
}

fn at(path: &str) -> &str {
    if path.is_empty() { "arguments" } else { path }
}

fn check(schema: &Value, v: &Value, path: &str, errors: &mut Vec<String>) {
    if errors.len() >= 20 {
        return;
    }
    let Some(s) = schema.as_object() else {
        if schema == &Value::Bool(false) {
            errors.push(format!("{}: not allowed", at(path)));
        }
        return;
    };
    if let Some(any) = s.get("anyOf").and_then(Value::as_array)
        && !any.iter().any(|sub| validate(sub, v).is_empty())
    {
        errors.push(format!("{}: does not match any allowed form", at(path)));
        return;
    }
    if let Some(t) = s.get("type") {
        let ok = match t {
            Value::String(t) => type_matches(t, v),
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).any(|t| type_matches(t, v)),
            _ => true,
        };
        if !ok {
            let want = match t {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            errors.push(format!("{}: expected {want}, got {}", at(path), type_name(v)));
            return;
        }
    }
    if let Some(e) = s.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        let allowed: Vec<String> = e.iter().map(|x| x.to_string()).collect();
        errors.push(format!("{}: must be one of {}", at(path), allowed.join(", ")));
    }
    if let Some(c) = s.get("const")
        && c != v
    {
        errors.push(format!("{}: must be {c}", at(path)));
    }
    if let Some(n) = v.as_f64().filter(|_| v.is_number()) {
        if let Some(min) = s.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            errors.push(format!("{}: must be >= {min}", at(path)));
        }
        if let Some(max) = s.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            errors.push(format!("{}: must be <= {max}", at(path)));
        }
    }
    if let Some(st) = v.as_str() {
        let len = st.chars().count();
        if let Some(min) = s.get("minLength").and_then(Value::as_u64)
            && (len as u64) < min
        {
            errors.push(format!("{}: must have at least {min} characters", at(path)));
        }
        if let Some(max) = s.get("maxLength").and_then(Value::as_u64)
            && len as u64 > max
        {
            errors.push(format!("{}: must have at most {max} characters (has {len})", at(path)));
        }
    }
    if let Some(arr) = v.as_array() {
        if let Some(min) = s.get("minItems").and_then(Value::as_u64)
            && (arr.len() as u64) < min
        {
            errors.push(format!("{}: needs at least {min} items", at(path)));
        }
        if let Some(max) = s.get("maxItems").and_then(Value::as_u64)
            && arr.len() as u64 > max
        {
            errors.push(format!("{}: at most {max} items allowed (got {})", at(path), arr.len()));
        }
        if let Some(items) = s.get("items") {
            for (i, x) in arr.iter().enumerate() {
                check(items, x, &format!("{path}[{i}]"), errors);
            }
        }
    }
    if let Some(obj) = v.as_object() {
        if let Some(req) = s.get("required").and_then(Value::as_array) {
            for r in req.iter().filter_map(Value::as_str) {
                if !obj.contains_key(r) {
                    let p = if path.is_empty() { r.to_string() } else { format!("{path}.{r}") };
                    errors.push(format!("{p}: required"));
                }
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        for (k, x) in obj {
            let p = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
            match props.and_then(|p| p.get(k)) {
                Some(sub) => check(sub, x, &p, errors),
                None => match s.get("additionalProperties") {
                    Some(Value::Bool(false)) => errors.push(format!("{p}: unknown argument")),
                    Some(sub) if sub.is_object() => check(sub, x, &p, errors),
                    _ => {}
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 8},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                "mode": {"type": "string", "enum": ["a", "b"]},
                "changes": {"type": "array", "maxItems": 2, "items": {
                    "type": "object", "required": ["path"], "additionalProperties": false,
                    "properties": {"path": {"type": "string"}, "executable": {"type": "boolean"}}}},
                "tag": {"type": ["string", "null"]},
                "either": {"anyOf": [{"type": "string"}, {"type": "integer"}]}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    #[test]
    fn accepts_a_valid_instance() {
        let ok = json!({"path": "a", "limit": 5, "mode": "a", "changes": [{"path": "x"}], "tag": null, "either": 3});
        assert!(validate(&schema(), &ok).is_empty());
        check_supported(&schema()).unwrap();
    }

    #[test]
    fn reports_every_kind_of_violation_with_a_path() {
        let bad = json!({"limit": 0.5, "mode": "c", "extra": 1, "changes": [{"path": 3}, {}, {"path": "z", "x": 1}], "tag": 4});
        let errs = validate(&schema(), &bad).join(" | ");
        for expect in [
            "path: required",
            "limit: expected integer, got number",
            "mode: must be one of \"a\", \"b\"",
            "extra: unknown argument",
            "changes: at most 2 items allowed (got 3)",
            "changes[0].path: expected string, got integer",
            "changes[1].path: required",
            "changes[2].x: unknown argument",
            "tag: expected [\"string\",\"null\"], got integer",
        ] {
            assert!(errs.contains(expect), "missing `{expect}` in: {errs}");
        }
    }

    #[test]
    fn bounds_and_lengths() {
        let errs = validate(&schema(), &json!({"path": "", "limit": 101})).join("|");
        assert!(errs.contains("path: must have at least 1 characters"), "{errs}");
        assert!(errs.contains("limit: must be <= 100"), "{errs}");
        let errs = validate(&schema(), &json!({"path": "123456789"})).join("|");
        assert!(errs.contains("at most 8 characters (has 9)"), "{errs}");
        assert!(validate(&schema(), &json!({"path": "é✓"})).is_empty(), "length counts characters, not bytes");
    }

    #[test]
    fn whole_numbers_written_as_floats_are_integers() {
        assert!(validate(&json!({"type": "integer"}), &json!(3.0)).is_empty());
        assert!(!validate(&json!({"type": "integer"}), &json!(3.5)).is_empty());
        assert!(!validate(&json!({"type": "integer"}), &json!("3")).is_empty());
    }

    #[test]
    fn unknown_keywords_are_rejected_at_build_time() {
        assert!(check_supported(&json!({"type": "string", "pattern": "^a"})).unwrap_err().contains("pattern"));
        assert!(check_supported(&json!({"type": "object", "properties": {"x": {"oneOf": []}}})).is_err());
    }

    #[test]
    fn a_non_object_instance_for_an_object_schema() {
        let errs = validate(&schema(), &json!("nope"));
        assert_eq!(errs, vec!["arguments: expected object, got string"]);
    }
}
