//! Compiles `proto/agwp/v1/agwp.proto` without needing an external `protoc`,
//! and derives `serde` and OpenAPI schemas for every message so the HTTP/JSON
//! transport serves exactly the same types as gRPC.

use std::path::PathBuf;

/// Fields that carry a JSON document inside a string; the HTTP/JSON mapping
/// exposes them as real JSON values under a friendlier name.
const JSON_FIELDS: &[(&str, &str)] = &[
    ("agwp.v1.Event.payload_json", "payload"),
    ("agwp.v1.Blocker.detail_json", "detail"),
    ("agwp.v1.Job.result_json", "result"),
    ("agwp.v1.CompleteJobRequest.result_json", "result"),
    ("agwp.v1.AuditEntry.details_json", "details"),
];

/// `bytes` fields: base64 in JSON.
const BYTES_FIELDS: &[&str] = &["agwp.v1.ObjectChunk.data", "agwp.v1.FileChange.content"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let proto_root = manifest.join("proto");
    let proto = proto_root.join("agwp/v1/agwp.proto");
    println!("cargo:rerun-if-changed={}", proto.display());

    let fds = protox::compile([&proto], [&proto_root])?;

    let mut builder =
        tonic_prost_build::configure().build_client(true).build_server(true).btree_map(".").type_attribute(
            ".agwp.v1",
            "#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)] #[serde(default)]",
        );
    for (path, json_name) in JSON_FIELDS {
        builder = builder.field_attribute(
            path,
            format!("#[serde(rename = \"{json_name}\", with = \"crate::json_string\")] #[schema(value_type = Object)]"),
        );
    }
    for path in BYTES_FIELDS {
        builder = builder.field_attribute(
            path,
            "#[serde(with = \"crate::serde_b64\")] #[schema(value_type = String, format = Byte)]",
        );
    }
    // Additive diff metadata is absent when a new client talks to an older server.
    for field in ["old_mode", "new_mode", "truncated"] {
        builder = builder.field_attribute(format!("agwp.v1.FileDiff.{field}"), "#[schema(required = false)]");
    }
    builder = builder.field_attribute("agwp.v1.ProposalDetail.requested_reviewers", "#[schema(required = false)]");
    builder.compile_fds(fds)?;
    Ok(())
}
