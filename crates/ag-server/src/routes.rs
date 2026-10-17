//! Route metadata and the generated OpenAPI document.
//!
//! The HTTP router and the OpenAPI description are built from the same route
//! table, and the message schemas come from the very Rust types that are
//! (de)serialized on the wire, so the published `openapi.json` cannot drift
//! from the implementation.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::Schema;
use utoipa::{PartialSchema, ToSchema};

type Collect = fn(&mut Vec<(String, RefOr<Schema>)>);

fn collect_for<T: ToSchema>(out: &mut Vec<(String, RefOr<Schema>)>) {
    out.push((T::name().to_string(), <T as PartialSchema>::schema()));
    T::schemas(out);
}

/// What a route accepts and returns.
#[derive(Clone)]
pub enum Payload {
    /// JSON request/response messages.
    Json {
        /// Request schema name.
        request: String,
        /// Response schema name.
        response: String,
        /// Collects the schemas used by both.
        collect: [Collect; 2],
    },
    /// Binary or streaming body described only in prose.
    Raw,
}

/// One HTTP route.
#[derive(Clone)]
pub struct RouteMeta {
    /// Lowercase HTTP method.
    pub method: &'static str,
    /// Path with `{placeholders}`.
    pub path: &'static str,
    /// Stable operation id (also the metrics label).
    pub operation: &'static str,
    /// Grouping tag.
    pub tag: &'static str,
    /// One-line description.
    pub summary: &'static str,
    /// True if no credentials are needed.
    pub public: bool,
    /// Body description.
    pub payload: Payload,
}

impl RouteMeta {
    /// A JSON route.
    pub fn json<Req: ToSchema, Resp: ToSchema>(
        method: &'static str,
        path: &'static str,
        operation: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self {
            method,
            path,
            operation,
            tag,
            summary,
            public: false,
            payload: Payload::Json {
                request: Req::name().to_string(),
                response: Resp::name().to_string(),
                collect: [collect_for::<Req>, collect_for::<Resp>],
            },
        }
    }

    /// A route with a raw (binary or event-stream) body.
    pub fn raw(
        method: &'static str,
        path: &'static str,
        operation: &'static str,
        tag: &'static str,
        summary: &'static str,
    ) -> Self {
        Self { method, path, operation, tag, summary, public: false, payload: Payload::Raw }
    }

    /// Marks the route as needing no credentials.
    #[must_use]
    pub fn public(mut self) -> Self {
        self.public = true;
        self
    }
}

fn placeholders(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
        .map(|s| s.trim_start_matches('*').to_string())
        .collect()
}

/// Builds the OpenAPI 3.1 document.
pub fn openapi(routes: &[RouteMeta], server_version: &str) -> Value {
    let mut schemas: BTreeMap<String, Value> = BTreeMap::new();
    for r in routes {
        if let Payload::Json { collect, .. } = &r.payload {
            let mut found = Vec::new();
            for c in collect {
                c(&mut found);
            }
            for (name, schema) in found {
                schemas.entry(name).or_insert_with(|| serde_json::to_value(schema).unwrap_or(Value::Null));
            }
        }
    }
    schemas.insert(
        "ErrorEnvelope".into(),
        json!({"type": "object", "properties": {"error": {"$ref": "#/components/schemas/ErrorInfo"}}, "required": ["error"]}),
    );

    let mut paths: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    for r in routes {
        let mut params: Vec<Value> = placeholders(r.path)
            .into_iter()
            .map(|p| {
                let (name, desc) = match p.as_str() {
                    "owner" => ("owner", "Repository owner (first half of owner/name)".to_string()),
                    "rname" => ("rname", "Repository name (second half of owner/name)".to_string()),
                    other => (other, String::new()),
                };
                json!({"name": name, "in": "path", "required": true, "description": desc, "schema": {"type": "string"}})
            })
            .collect();
        let mut op = json!({
            "operationId": r.operation,
            "tags": [r.tag],
            "summary": r.summary,
            "responses": {
                "default": {
                    "description": "Error",
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ErrorEnvelope"}}}
                }
            }
        });
        match &r.payload {
            Payload::Json { request, response, .. } => {
                if matches!(r.method, "get" | "delete") {
                    // Query parameters are the request message's own fields.
                    let path_names = placeholders(r.path);
                    if let Some(props) =
                        schemas.get(request).and_then(|s| s.get("properties")).and_then(Value::as_object)
                    {
                        for (k, v) in props {
                            let skip =
                                path_names.contains(k) || (k == "repo" && path_names.iter().any(|p| p == "owner"));
                            if !skip {
                                params.push(json!({"name": k, "in": "query", "required": false, "schema": v}));
                            }
                        }
                    }
                } else {
                    op["requestBody"] = json!({
                        "required": true,
                        "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{request}")}}}
                    });
                }
                op["responses"]["200"] = json!({
                    "description": "Success",
                    "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{response}")}}}
                });
            }
            Payload::Raw => {
                op["responses"]["200"] = json!({"description": "Success (binary or event-stream body)"});
            }
        }
        if !params.is_empty() {
            op["parameters"] = Value::Array(params);
        }
        if r.public {
            op["security"] = json!([]);
        }
        paths.entry(r.path.to_string()).or_default().insert(r.method.to_string(), op);
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "AgentGit AGWP HTTP/JSON API",
            "version": server_version,
            "description": "The HTTP/JSON gateway of AGWP v1. The same messages are served over gRPC; see proto/agwp/v1/agwp.proto. \
                            Mutating requests accept an `Idempotency-Key` header. Free-form names (streams, revisions, paths) travel in the query string or body.",
            "license": {"name": "Apache-2.0", "url": "https://www.apache.org/licenses/LICENSE-2.0.html"}
        },
        "servers": [{"url": "/"}],
        "security": [{"bearerAuth": []}],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "bearerAuth": {"type": "http", "scheme": "bearer", "description": "A scoped token (agt_…), an OIDC token, or the bootstrap token."}
            },
            "schemas": schemas
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_extracted() {
        assert_eq!(placeholders("/v1/repos/{owner}/{rname}/files/{*path}"), ["owner", "rname", "path"]);
        assert!(placeholders("/v1/repos").is_empty());
    }
}
