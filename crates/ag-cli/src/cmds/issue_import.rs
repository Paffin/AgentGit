//! Explicit preview/apply for a normalized hosting issue manifest.
use std::io::Read;
use std::path::PathBuf;

use ag_client::{Client, ClientError};
use ag_protocol::{features, v1};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde_json::Value;

/// Inspect an import first, then apply precisely the reviewed manifest digest.
#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Normalized GitHub/GitLab issue manifest (UTF-8 JSON, at most 4 MiB).
    #[arg(long, value_name = "PATH")]
    file: PathBuf,
    /// Apply the manifest instead of only previewing it.
    #[arg(long, requires = "expected_digest")]
    pub apply: bool,
    /// Exact digest returned by the preview of this manifest.
    #[arg(long, requires = "apply", value_name = "DIGEST")]
    expected_digest: Option<String>,
    /// Acknowledge unsupported source features listed in the preview.
    #[arg(long, requires = "apply")]
    allow_unsupported: bool,
}

fn manifest(path: &PathBuf) -> Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("reading {}", path.display()))?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(bytes.len() <= 4 * 1024 * 1024, "issue import manifest exceeds 4 MiB");
    String::from_utf8(bytes).context("issue import manifest must be UTF-8 JSON")
}

/// Preview by default; applying never fetches a replacement digest implicitly.
pub async fn run(client: &Client, repo: &str, args: &ImportArgs) -> Result<Value> {
    ensure!(
        client.negotiate().await?.features.iter().any(|feature| feature == features::ISSUES_IMPORT),
        "this server does not support issue imports; update it before using this command"
    );
    let manifest_json = manifest(&args.file)?;
    if args.apply {
        let response = client
            .issues()
            .apply_import(client.req(v1::ApplyIssueImportRequest {
                repo: repo.into(),
                manifest_json,
                expected_digest: args.expected_digest.clone().context("--apply requires --expected-digest")?,
                allow_unsupported: args.allow_unsupported,
            }))
            .await
            .map_err(ClientError::from)?
            .into_inner();
        Ok(serde_json::to_value(response)?)
    } else {
        let response = client
            .issues()
            .preview_import(client.req(v1::PreviewIssueImportRequest { repo: repo.into(), manifest_json }))
            .await
            .map_err(ClientError::from)?
            .into_inner();
        Ok(serde_json::to_value(response)?)
    }
}

/// Keep the reviewed digest and any refusal reasons visible in terminal output.
pub fn describe(value: &Value, applied: bool) -> String {
    if applied {
        return format!(
            "Imported {} issues; skipped {} already imported issues.\n{}",
            value["created"],
            value["skipped"],
            value["mappings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|mapping| format!(
                    "{} -> #{}",
                    mapping["source_id"].as_str().unwrap_or_default(),
                    mapping["number"]
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
    let mut text = format!(
        "Preview: {} issues, {} comments. Create {}, skip {}.\nDigest: {}",
        value["issues"],
        value["comments"],
        value["create_count"],
        value["skip_count"],
        value["digest"].as_str().unwrap_or_default()
    );
    for (key, label) in [("warnings", "Warning"), ("conflicts", "Conflict")] {
        for message in value[key].as_array().into_iter().flatten() {
            text.push_str(&format!(
                "\n{label}: {}",
                message.as_str().map(str::to_owned).unwrap_or_else(|| message.to_string())
            ));
        }
    }
    text.push_str("\nTo apply this preview, add --apply --expected-digest DIGEST using the value above.");
    text
}
