//! Fetch a bounded, credential-free issue manifest for a later explicit import.
use std::{collections::BTreeSet, io::Read, path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_ISSUES: usize = 100;
const MAX_COMMENTS: usize = 1000;
const PAGE_SIZE: usize = 100;

#[derive(Clone, Copy, Debug, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Github,
    Gitlab,
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Source hosting provider.
    #[arg(long, value_enum)]
    pub provider: Provider,
    /// Repository path, for example owner/repo or group/subgroup/repo.
    #[arg(long)]
    pub source: String,
    /// GitLab HTTPS origin, for example https://gitlab.example.com.
    #[arg(long)]
    pub host: Option<String>,
    /// New manifest file. An existing file is never replaced.
    #[arg(long)]
    pub output: PathBuf,
}

#[derive(Serialize)]
pub struct ExportReport {
    pub issues: usize,
    pub comments: usize,
    pub output: PathBuf,
    pub unsupported: Vec<String>,
}

#[derive(Serialize)]
struct Manifest {
    version: u8,
    provider: Provider,
    source_repo: String,
    unsupported: Vec<String>,
    issues: Vec<Issue>,
}
#[derive(Serialize)]
struct Issue {
    source_id: String,
    title: String,
    body: String,
    state: &'static str,
    author: String,
    url: String,
    created_at_ms: i64,
    updated_at_ms: i64,
    comments: Vec<Comment>,
}
#[derive(Serialize)]
struct Comment {
    source_id: String,
    body: String,
    author: String,
    url: String,
    created_at_ms: i64,
}
#[derive(Deserialize, Default)]
struct User {
    #[serde(default)]
    login: String,
    #[serde(default)]
    username: String,
}
impl User {
    fn name(self) -> String {
        if self.login.is_empty() { self.username } else { self.login }
    }
}
#[derive(Deserialize)]
struct SourceIssue {
    #[serde(default)]
    number: u64,
    #[serde(default)]
    iid: u64,
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    description: Option<String>,
    state: String,
    #[serde(default)]
    user: Option<User>,
    #[serde(default)]
    author: Option<User>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    web_url: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}
#[derive(Deserialize)]
struct SourceComment {
    id: u64,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<User>,
    #[serde(default)]
    author: Option<User>,
    #[serde(default)]
    html_url: String,
    created_at: String,
    #[serde(default)]
    system: bool,
}

struct Source {
    provider: Provider,
    api: Url,
    repository: Url,
    token: Option<String>,
    agent: ureq::Agent,
    remaining: usize,
}
impl Source {
    fn new(args: &ExportArgs) -> Result<Self> {
        let parts: Vec<_> = args.source.split('/').collect();
        ensure!(
            parts.len() >= 2
                && parts.iter().all(|p| !p.is_empty()
                    && *p != "."
                    && *p != ".."
                    && !p.contains(['?', '#', ':', '\\'])
                    && !p.chars().any(char::is_control)),
            "source must be a repository path such as owner/repo"
        );
        let (origin, api_origin, variable) = match args.provider {
            Provider::Github => {
                ensure!(
                    parts.len() == 2 && args.host.is_none(),
                    "GitHub requires owner/repo; --host is supported only for GitLab"
                );
                ("https://github.com", "https://api.github.com", "GITHUB_TOKEN")
            }
            Provider::Gitlab => {
                let host = args.host.as_deref().unwrap_or("https://gitlab.com");
                (host, host, "GITLAB_TOKEN")
            }
        };
        let mut repository = https_origin(origin)?;
        repository.path_segments_mut().map_err(|_| anyhow::anyhow!("invalid source host"))?.extend(&parts);
        let mut api = https_origin(api_origin)?;
        match args.provider {
            Provider::Github => {
                api.path_segments_mut().unwrap().extend(["repos", parts[0], parts[1]]);
            }
            Provider::Gitlab => {
                api.path_segments_mut().unwrap().extend(["api", "v4", "projects", &args.source]);
            }
        }
        let token = std::env::var(variable).ok().filter(|value| !value.is_empty());
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .into();
        Ok(Self { provider: args.provider, api, repository, token, agent, remaining: MAX_BYTES })
    }
    fn endpoint(&self, segments: &[&str], page: usize) -> Url {
        let mut url = self.api.clone();
        url.path_segments_mut().unwrap().extend(segments);
        url.query_pairs_mut().append_pair("per_page", "100").append_pair("page", &page.to_string());
        url
    }
    fn page<T: for<'de> Deserialize<'de>>(&mut self, url: &Url) -> Result<Vec<T>> {
        let mut request = ureq::http::Request::builder()
            .uri(url.as_str())
            .header("User-Agent", "AgentGit-issue-export")
            .header("Accept", "application/json");
        if let Some(token) = &self.token {
            request = match self.provider {
                Provider::Github => request.header("Authorization", format!("Bearer {token}")),
                Provider::Gitlab => request.header("PRIVATE-TOKEN", token),
            };
        }
        let request = request.body(()).map_err(|_| anyhow::anyhow!("invalid provider request or token header"))?;
        let mut response = self
            .agent
            .run(request)
            .map_err(|_| anyhow::anyhow!("provider request failed; check network and source host"))?;
        ensure!(
            response.status().is_success(),
            "provider returned HTTP {}; check repository access, token and rate limits",
            response.status().as_u16()
        );
        let mut bytes = Vec::new();
        std::io::Read::take(response.body_mut().with_config().limit(u64::MAX).reader(), self.remaining as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow::anyhow!("could not read provider response"))?;
        ensure!(
            bytes.len() <= self.remaining,
            "provider responses exceed the 4 MiB export budget; export a smaller source"
        );
        self.remaining -= bytes.len();
        serde_json::from_slice(&bytes).context("provider returned invalid issue data")
    }
    fn issues(&mut self) -> Result<Vec<SourceIssue>> {
        let mut issues = Vec::new();
        for page in 1..=100 {
            let mut url = self.endpoint(&["issues"], page);
            url.query_pairs_mut().append_pair("state", "all");
            match self.provider {
                Provider::Github => {
                    url.query_pairs_mut().append_pair("sort", "created").append_pair("direction", "asc");
                }
                Provider::Gitlab => {
                    url.query_pairs_mut()
                        .append_pair("scope", "all")
                        .append_pair("order_by", "iid")
                        .append_pair("sort", "asc");
                }
            }
            let rows: Vec<SourceIssue> = self.page(&url)?;
            let count = rows.len();
            for row in rows {
                if matches!(self.provider, Provider::Github) && row.pull_request.is_some() {
                    continue;
                }
                ensure!(issues.len() < MAX_ISSUES, "source exceeds 100 issues; export a smaller source");
                issues.push(row);
            }
            if count < PAGE_SIZE {
                return Ok(issues);
            }
        }
        bail!("provider pagination exceeds the export budget")
    }
    fn comments(&mut self, number: u64, total: &mut usize) -> Result<Vec<Comment>> {
        let mut comments = Vec::new();
        let mut seen = BTreeSet::new();
        for page in 1..=11 {
            let number_text = number.to_string();
            let suffix = match self.provider {
                Provider::Github => "comments",
                Provider::Gitlab => "notes",
            };
            let mut url = self.endpoint(&["issues", &number_text, suffix], page);
            if matches!(self.provider, Provider::Gitlab) {
                url.query_pairs_mut().append_pair("order_by", "created_at").append_pair("sort", "asc");
            }
            let rows: Vec<SourceComment> = self.page(&url)?;
            let count = rows.len();
            for row in rows {
                if row.system {
                    continue;
                }
                ensure!(*total < MAX_COMMENTS, "source exceeds 1000 comments; export a smaller source");
                ensure!(
                    row.id > 0 && seen.insert(row.id),
                    "provider returned a missing or duplicate comment ID; retry after source activity settles"
                );
                let url = if matches!(self.provider, Provider::Github) {
                    row.html_url
                } else {
                    format!("{}/-/issues/{number}#note_{}", self.repository, row.id)
                };
                comments.push(Comment {
                    source_id: row.id.to_string(),
                    body: row.body.unwrap_or_default(),
                    author: row.user.or(row.author).unwrap_or_default().name(),
                    url,
                    created_at_ms: timestamp(&row.created_at)?,
                });
                *total += 1;
            }
            if count < PAGE_SIZE {
                return Ok(comments);
            }
        }
        bail!("provider comment pagination exceeds the export budget")
    }
}
fn https_origin(value: &str) -> Result<Url> {
    let url = Url::parse(value).context("host must be an HTTPS origin")?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "host must be an HTTPS origin without credentials, path, query or fragment"
    );
    Ok(url)
}
fn timestamp(value: &str) -> Result<i64> {
    let date = OffsetDateTime::parse(value, &Rfc3339).context("provider returned an invalid RFC3339 timestamp")?;
    i64::try_from(date.unix_timestamp_nanos() / 1_000_000).context("provider timestamp is out of range")
}

/// Export only; no AgentGit server call or issue publication occurs here.
pub fn export_issues(args: &ExportArgs) -> Result<ExportReport> {
    ensure!(!args.output.exists(), "output already exists; choose a new manifest file");
    let mut source = Source::new(args)?;
    let rows = source.issues()?;
    let mut issues = Vec::new();
    let mut total = 0;
    let mut seen = BTreeSet::new();
    for row in rows {
        let number = match args.provider {
            Provider::Github => row.number,
            Provider::Gitlab => row.iid,
        };
        ensure!(
            number > 0 && seen.insert(number),
            "provider returned a missing or duplicate issue ID; retry after source activity settles"
        );
        let state = match row.state.as_str() {
            "open" | "opened" => "open",
            "closed" => "closed",
            _ => bail!("provider returned an unsupported issue state"),
        };
        let comments = source.comments(number, &mut total)?;
        issues.push(Issue {
            source_id: number.to_string(),
            title: row.title,
            body: row.body.or(row.description).unwrap_or_default(),
            state,
            author: row.user.or(row.author).unwrap_or_default().name(),
            url: if matches!(args.provider, Provider::Github) { row.html_url } else { row.web_url },
            created_at_ms: timestamp(&row.created_at)?,
            updated_at_ms: timestamp(&row.updated_at)?,
            comments,
        });
    }
    let mut unsupported = vec![
        "Labels, assignees, milestones and reactions are not imported.".into(),
        "Attachment files are not downloaded; links remain in the original body.".into(),
        "Pull/merge requests, releases, permissions and edit history are not included.".into(),
    ];
    if matches!(args.provider, Provider::Gitlab) {
        unsupported.push("GitLab system notes are omitted; only user comments are exported.".into());
    }
    let manifest = Manifest {
        version: 1,
        provider: args.provider,
        source_repo: source.repository.to_string(),
        unsupported: unsupported.clone(),
        issues,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    ensure!(bytes.len() <= MAX_BYTES, "manifest exceeds 4 MiB; export a smaller source");
    let parent =
        args.output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent).context("creating manifest file")?;
    std::io::Write::write_all(&mut file, &bytes).context("writing manifest")?;
    file.persist_noclobber(&args.output)
        .map_err(|e| e.error)
        .context("saving manifest without replacing existing data")?;
    Ok(ExportReport { issues: manifest.issues.len(), comments: total, output: args.output.clone(), unsupported })
}
