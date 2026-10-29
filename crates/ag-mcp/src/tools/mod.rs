//! The tool registry: definitions (name, group, schemas, annotations) and the context handlers run in.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value, json};

use crate::api::{Api, Opts};
use crate::config::{Config, Group, check_repo_name};
use crate::output::{ToolError, ToolOutput};

pub mod coordinate;
pub mod discussions;
pub mod proposals;
pub mod read;
pub mod workspace;

/// The signature of a tool implementation. `args` already passed schema validation.
pub type Handler = Box<dyn Fn(&CallCtx<'_>, &Value) -> Result<ToolOutput, ToolError> + Send + Sync>;

/// Honest behaviour hints (`annotations` of a tool).
#[derive(Clone, Copy, Debug)]
pub struct Ann {
    /// Changes nothing.
    pub read_only: bool,
    /// May destroy or overwrite something that cannot be recovered by reading history.
    pub destructive: bool,
    /// Repeating the call with the same arguments has no further effect.
    pub idempotent: bool,
}

impl Ann {
    /// A pure read.
    pub const READ: Ann = Ann { read_only: true, destructive: false, idempotent: true };
    /// Adds something, destroys nothing, repeating is not a no-op.
    pub const ADD: Ann = Ann { read_only: false, destructive: false, idempotent: false };
    /// Changes something to a state that repeating leaves unchanged.
    pub const SET: Ann = Ann { read_only: false, destructive: false, idempotent: true };
    /// Removes or finally decides something.
    pub const DESTROY: Ann = Ann { read_only: false, destructive: true, idempotent: true };
}

/// A tool.
pub struct ToolDef {
    /// Stable name.
    pub name: &'static str,
    /// Display name.
    pub title: &'static str,
    /// What it does, for the model.
    pub description: String,
    /// The group that switches it on.
    pub group: Group,
    /// Behaviour hints.
    pub ann: Ann,
    /// JSON Schema of the arguments.
    pub input: Value,
    /// JSON Schema of `structuredContent`.
    pub output: Value,
    /// Implementation.
    pub handler: Handler,
}

impl ToolDef {
    /// The entry of `tools/list`.
    pub fn to_json(&self, structured: bool) -> Value {
        let mut o = Map::new();
        o.insert("name".into(), json!(self.name));
        if structured {
            o.insert("title".into(), json!(self.title));
        }
        o.insert("description".into(), json!(self.description));
        o.insert("inputSchema".into(), self.input.clone());
        if structured {
            o.insert("outputSchema".into(), self.output.clone());
        }
        o.insert(
            "annotations".into(),
            json!({
                "title": self.title,
                "readOnlyHint": self.ann.read_only,
                "destructiveHint": self.ann.destructive,
                "idempotentHint": self.ann.idempotent,
                "openWorldHint": false,
            }),
        );
        Value::Object(o)
    }
}

/// All tools, in a stable order, and the allow-list that decides which are offered.
pub struct Registry {
    tools: Vec<ToolDef>,
    index: BTreeMap<&'static str, usize>,
}

impl Registry {
    /// Builds the registry of every tool of this crate.
    pub fn standard() -> Self {
        let mut tools = Vec::new();
        tools.extend(read::tools());
        tools.extend(workspace::tools());
        tools.extend(proposals::tools());
        tools.extend(discussions::tools());
        tools.extend(coordinate::tools());
        Self::from_tools(tools)
    }

    /// A registry of the given tools (tests add their own).
    pub fn from_tools(tools: Vec<ToolDef>) -> Self {
        let index = tools.iter().enumerate().map(|(i, t)| (t.name, i)).collect();
        Self { tools, index }
    }

    /// Looks a tool up by name, whether or not it is enabled.
    pub fn get(&self, name: &str) -> Option<&ToolDef> {
        self.index.get(name).map(|i| &self.tools[*i])
    }

    /// The tools the allow-list switches on, in order.
    pub fn enabled<'a>(&'a self, cfg: &'a Config) -> impl Iterator<Item = &'a ToolDef> {
        self.tools.iter().filter(|t| cfg.allows(t.group))
    }

    /// Every tool, enabled or not.
    pub fn all(&self) -> &[ToolDef] {
        &self.tools
    }
}

/// Progress reporting: `(progress, total, message)`.
pub type Progress = dyn Fn(f64, Option<f64>, &str) + Send + Sync;

/// What a handler may use.
pub struct CallCtx<'a> {
    /// Operator configuration.
    pub cfg: &'a Config,
    /// The AGWP client.
    pub api: &'a Api,
    /// Set when the MCP client cancelled this call.
    pub cancel: &'a AtomicBool,
    /// Progress reporting (`progress`, `total`, `message`); a no-op when the client sent no progress token.
    pub progress: &'a Progress,
}

impl<'a> CallCtx<'a> {
    /// Fails with `cancelled` if the client cancelled the request.
    pub fn check_cancelled(&self) -> Result<(), ToolError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(ToolError::local("cancelled", "the call was cancelled"))
        } else {
            Ok(())
        }
    }

    /// Request options for an optional run.
    pub fn opts<'b>(&'b self, run_id: Option<&'b str>) -> Opts<'b> {
        Opts { run_id, idempotency_key: None, cancel: Some(self.cancel) }
    }

    /// Request options for a creation that is safe to repeat.
    pub fn opts_keyed<'b>(&'b self, run_id: Option<&'b str>, key: &'b str) -> Opts<'b> {
        Opts { run_id, idempotency_key: Some(key), cancel: Some(self.cancel) }
    }

    /// The repository of this call: the argument, or the one the operator pinned.
    pub fn repo(&self, args: &Value) -> Result<String, ToolError> {
        let repo = match str_arg(args, "repo") {
            Some(r) => r.to_string(),
            None => self
                .cfg
                .pinned_repo()
                .ok_or_else(|| {
                    ToolError::invalid("`repo` (owner/name) is required: the operator did not pin a single repository")
                        .hint("Use repo_list to see the repositories you can use.")
                })?
                .to_string(),
        };
        check_repo_name(&repo).map_err(ToolError::invalid)?;
        if !self.cfg.repo_allowed(&repo) {
            return Err(ToolError::refused(
                "repo_not_allowed",
                format!(
                    "The repository `{repo}` is outside the allow-list of this MCP server ({}). Nothing was sent to AgentGit.",
                    self.cfg.repos.join(", ")
                ),
            )
            .hint("Only the repositories the operator listed with --repo can be used in this session."));
        }
        Ok(repo)
    }

    /// The `run_id` argument, validated (it becomes a header).
    pub fn run_id<'b>(&self, args: &'b Value) -> Result<Option<&'b str>, ToolError> {
        match str_arg(args, "run_id") {
            None => Ok(None),
            Some(r)
                if !r.is_empty()
                    && r.len() <= 80
                    && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') =>
            {
                Ok(Some(r))
            }
            Some(_) => Err(ToolError::invalid(
                "`run_id` must be the id returned by task_claim or run_start (letters, digits, _ and -)",
            )),
        }
    }

    /// The repository's default stream.
    pub fn default_stream(&self, repo: &str, run_id: Option<&str>) -> Result<String, ToolError> {
        let v = self.api.get(&crate::api::repo_path(repo), &[], &self.opts(run_id))?;
        let s = v.get("default_stream").and_then(Value::as_str).unwrap_or("");
        if s.is_empty() {
            return Err(ToolError::local("not_found", "the repository has no default stream; pass `stream` or `rev`"));
        }
        Ok(s.to_string())
    }

    /// `rev` argument, or the default stream.
    pub fn rev_or_default(&self, repo: &str, args: &Value, key: &str) -> Result<String, ToolError> {
        match str_arg(args, key) {
            Some(r) if !r.is_empty() => Ok(r.to_string()),
            _ => self.default_stream(repo, None),
        }
    }

    /// Reports progress (`step` of `total`) if the client asked for it.
    pub fn step(&self, step: u32, total: u32, message: &str) {
        (self.progress)(f64::from(step), Some(f64::from(total)), message);
    }
}

// ---------------------------------------------------------------------------------------------------------
// argument helpers

/// A string argument.
pub fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// A non-negative integer argument.
pub fn u64_arg(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}

/// A boolean argument with a default.
pub fn bool_arg(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// A short hex prefix of an id for display.
pub fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

/// Bounds a free-text field of a list item so that one long message cannot use up the output budget.
pub fn bounded(s: &str, max: usize) -> String {
    crate::sanitize::bound_text(s, max).0
}

// ---------------------------------------------------------------------------------------------------------
// schema helpers

/// `{"type":"string","description":…}`.
pub fn t_str(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

/// A string with a length bound.
pub fn t_str_max(desc: &str, max: usize) -> Value {
    json!({"type": "string", "description": desc, "maxLength": max})
}

/// An integer in a range.
pub fn t_int(desc: &str, min: i64, max: i64) -> Value {
    json!({"type": "integer", "description": desc, "minimum": min, "maximum": max})
}

/// A boolean.
pub fn t_bool(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}

/// A string from a fixed set.
pub fn t_enum(desc: &str, values: &[&str]) -> Value {
    json!({"type": "string", "description": desc, "enum": values})
}

/// An array.
pub fn t_arr(desc: &str, items: Value, max: usize) -> Value {
    json!({"type": "array", "description": desc, "items": items, "maxItems": max})
}

/// An input schema: an object that accepts only the listed properties.
pub fn input(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut p = Map::new();
    for (k, v) in props {
        p.insert(k.into(), v);
    }
    json!({"type": "object", "properties": p, "required": required, "additionalProperties": false})
}

/// An output schema: an object with some required, typed members (others, such as `truncated` or `sanitized`, may appear).
pub fn output(props: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut p = Map::new();
    for (k, v) in props {
        p.insert(k.into(), v);
    }
    json!({"type": "object", "properties": p, "required": required})
}

/// The `repo` argument shared by most tools.
pub fn p_repo() -> (&'static str, Value) {
    ("repo", t_str("Repository as owner/name. Optional when the operator pinned a single repository."))
}

/// The `run_id` argument shared by tools that act as an agent run.
pub fn p_run() -> (&'static str, Value) {
    (
        "run_id",
        t_str(
            "The run to act as (the run_id returned by task_claim or run_start). Optional: without it the call is made as the credential's own identity.",
        ),
    )
}

/// A loosely typed member of an output schema.
pub fn any() -> Value {
    json!({})
}

/// An output member that is an array.
pub fn o_arr() -> Value {
    json!({"type": "array"})
}

/// An output member that is an object.
pub fn o_obj() -> Value {
    json!({"type": "object"})
}

/// An output member that is a string.
pub fn o_str() -> Value {
    json!({"type": "string"})
}

/// An output member that is a boolean.
pub fn o_bool() -> Value {
    json!({"type": "boolean"})
}

/// An output member that is an integer.
pub fn o_int() -> Value {
    json!({"type": "integer"})
}

/// Builds a definition.
#[allow(clippy::too_many_arguments)]
pub fn def(
    name: &'static str,
    group: Group,
    title: &'static str,
    description: &str,
    ann: Ann,
    input: Value,
    output: Value,
    handler: impl Fn(&CallCtx<'_>, &Value) -> Result<ToolOutput, ToolError> + Send + Sync + 'static,
) -> ToolDef {
    ToolDef { name, title, description: description.to_string(), group, ann, input, output, handler: Box::new(handler) }
}

/// Unix time in milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Seconds from now until `at_ms` (negative if past), or `None` for `0` (never).
pub fn seconds_until(at_ms: i64) -> Option<i64> {
    (at_ms > 0).then(|| (at_ms - now_ms()) / 1000)
}

/// A random idempotency key.
pub fn new_key(prefix: &str) -> String {
    format!("{prefix}-{}", crate::sanitize::nonce())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{check_supported, validate};

    #[test]
    fn every_schema_uses_only_enforced_keywords_and_accepts_an_empty_valid_call() {
        let reg = Registry::standard();
        assert!(reg.all().len() >= 28, "{}", reg.all().len());
        let mut names = std::collections::BTreeSet::new();
        for t in reg.all() {
            assert!(names.insert(t.name), "duplicate tool {}", t.name);
            assert!(t.name.len() <= 64 && t.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{}", t.name);
            check_supported(&t.input).unwrap_or_else(|e| panic!("{} input: {e}", t.name));
            check_supported(&t.output).unwrap_or_else(|e| panic!("{} output: {e}", t.name));
            assert_eq!(t.input["additionalProperties"], false, "{} must refuse unknown arguments", t.name);
            assert!(t.description.len() > 40, "{} needs a real description", t.name);
            // required arguments are all declared properties
            for r in t.input["required"].as_array().unwrap() {
                assert!(t.input["properties"].get(r.as_str().unwrap()).is_some(), "{}: {r}", t.name);
            }
            // read-only tools never claim to be destructive
            if t.ann.read_only {
                assert!(!t.ann.destructive, "{}", t.name);
            }
        }
        // a read tool with no required argument validates `{}`
        assert!(validate(&reg.get("whoami").unwrap().input, &json!({})).is_empty());
    }

    #[test]
    fn annotations_are_honest() {
        let reg = Registry::standard();
        for t in reg.all() {
            match t.group {
                Group::Read => assert!(t.ann.read_only, "{} is in the read group so it must be read-only", t.name),
                Group::Review if matches!(t.name, "review_draft_get" | "comment_draft_get") => {
                    // Private notes require the review opt-in even though reading does not mutate them.
                    assert!(t.ann.read_only);
                }
                _ => assert!(!t.ann.read_only, "{} changes state so it cannot be marked read-only", t.name),
            }
        }
        let destructive: Vec<&str> = reg.all().iter().filter(|t| t.ann.destructive).map(|t| t.name).collect();
        assert_eq!(
            destructive,
            ["workspace_delete", "proposal_close", "review_draft_clear", "comment_draft_clear", "integrate"],
            "{destructive:?}"
        );
    }

    #[test]
    fn groups_decide_what_is_listed() {
        let reg = Registry::standard();
        let cfg = Config::new("http://127.0.0.1:1", crate::config::TokenSource::Fixed("t".into()));
        let names: Vec<&str> = reg.enabled(&cfg).map(|t| t.name).collect();
        assert!(names.contains(&"log") && names.contains(&"propose") && names.contains(&"lease_acquire"));
        assert!(!names.contains(&"review") && !names.contains(&"integrate"));
        let mut cfg = cfg;
        cfg.groups = [Group::Read].into_iter().collect();
        assert!(reg.enabled(&cfg).all(|t| t.group == Group::Read));
    }
}
