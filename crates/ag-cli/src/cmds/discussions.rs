//! The same revision-bound discussions used by the Web UI, over AGWP.
use ag_client::{Client, ClientError};
use ag_protocol::v1;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use serde_json::Value;

#[derive(Subcommand, Debug)]
pub enum DiscussionCmd {
    /// List line discussions, including resolved and earlier-head threads.
    List {
        /// Proposal id.
        id: String,
        /// Continue a preceding response.
        #[arg(long, default_value = "")]
        page_token: String,
    },
    /// Read replies to a line discussion.
    Replies {
        /// Proposal id.
        id: String,
        /// Thread id returned by list or start.
        thread: String,
        /// Continue a preceding response.
        #[arg(long, default_value = "")]
        page_token: String,
    },
    /// Start a thread on the exact revision and line you inspected.
    Start(StartArgs),
    /// Reply to an open line discussion.
    Reply {
        /// Proposal id.
        id: String,
        /// Thread id.
        thread: String,
        /// Reply text.
        #[arg(short, long)]
        message: String,
    },
    /// Resolve or reopen a thread only at the version you read.
    Resolve {
        /// Proposal id.
        id: String,
        /// Thread id.
        thread: String,
        /// Version from the latest discussion list response.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        expected_version: u64,
        /// Reopen instead of resolving.
        #[arg(long)]
        reopen: bool,
    },
}
#[derive(Args, Debug)]
pub struct StartArgs {
    /// Proposal id.
    id: String,
    /// Exact proposal head you inspected.
    #[arg(long)]
    head: String,
    /// Repository-relative file path.
    #[arg(long)]
    path: String,
    /// One-based line number.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    line: u32,
    /// Side of the diff. Old-side lines require --base.
    #[arg(long, default_value = "new", value_parser = ["old", "new"])]
    side: String,
    /// Exact base changeset used for the comparison.
    #[arg(long, default_value = "")]
    base: String,
    /// Initial discussion text.
    #[arg(short, long)]
    message: String,
}
impl DiscussionCmd {
    pub fn changes_state(&self) -> bool {
        !matches!(self, Self::List { .. } | Self::Replies { .. })
    }
}
fn next_page(token: &str) -> String {
    if token.is_empty() { String::new() } else { format!("\nMore results: repeat with --page-token {token}") }
}
fn summary(thread: &v1::ReviewThread) -> String {
    let anchor = thread.anchor.as_ref();
    format!(
        "{}  {}  version {}  {}:{} ({})\n  {}",
        thread.id,
        if thread.resolved { "resolved" } else { "open" },
        thread.version,
        anchor.map(|a| a.path.as_str()).unwrap_or("?"),
        anchor.map(|a| a.line).unwrap_or(0),
        anchor.map(|a| a.side.as_str()).unwrap_or("?"),
        thread.body
    )
}
async fn list(client: &Client, repo: &str, id: &str, token: &str) -> Result<(Value, String)> {
    let page = client
        .proposals()
        .list_threads(client.req(v1::ListReviewThreadsRequest {
            repo: repo.into(),
            id: id.into(),
            page_size: 50,
            page_token: token.into(),
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let text = if page.threads.is_empty() {
        "No line discussions in this page.".into()
    } else {
        page.threads.iter().map(summary).collect::<Vec<_>>().join("\n")
    };
    Ok((serde_json::to_value(&page)?, text + &next_page(&page.next_page_token)))
}
async fn replies(client: &Client, repo: &str, id: &str, thread: &str, token: &str) -> Result<(Value, String)> {
    let page = client
        .proposals()
        .list_thread_comments(client.req(v1::ListThreadCommentsRequest {
            repo: repo.into(),
            id: id.into(),
            thread_id: thread.into(),
            page_size: 50,
            page_token: token.into(),
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let text = if page.comments.is_empty() {
        "No replies in this page.".into()
    } else {
        page.comments.iter().map(|r| format!("#{} {}: {}", r.id, r.author, r.body)).collect::<Vec<_>>().join("\n")
    };
    Ok((serde_json::to_value(&page)?, text + &next_page(&page.next_page_token)))
}
async fn start(client: &Client, repo: &str, args: &StartArgs) -> Result<(Value, String)> {
    let request = v1::CreateReviewThreadRequest {
        repo: repo.into(),
        id: args.id.clone(),
        anchor: Some(v1::ReviewLineAnchor {
            head: args.head.clone(),
            base: args.base.clone(),
            path: args.path.clone(),
            side: args.side.clone(),
            line: args.line,
            text: String::new(),
        }),
        body: args.message.clone(),
        expected_head: args.head.clone(),
        idempotency_key: ag_crypto::random_id("thread")?,
    };
    let thread = client
        .retrying(|| async { client.proposals().create_thread(client.req(request.clone())).await })
        .await?
        .into_inner();
    Ok((serde_json::to_value(&thread)?, summary(&thread)))
}
async fn reply(client: &Client, repo: &str, id: &str, thread: &str, body: &str) -> Result<(Value, String)> {
    let request = v1::AddThreadCommentRequest {
        repo: repo.into(),
        id: id.into(),
        thread_id: thread.into(),
        body: body.into(),
        idempotency_key: ag_crypto::random_id("reply")?,
    };
    let reply = client
        .retrying(|| async { client.proposals().reply_thread(client.req(request.clone())).await })
        .await?
        .into_inner();
    Ok((serde_json::to_value(&reply)?, format!("Reply #{} posted to {}.", reply.id, reply.thread_id)))
}
async fn resolve(
    client: &Client,
    repo: &str,
    id: &str,
    thread: &str,
    version: u64,
    reopen: bool,
) -> Result<(Value, String)> {
    let request = v1::ResolveReviewThreadRequest {
        repo: repo.into(),
        id: id.into(),
        thread_id: thread.into(),
        resolved: !reopen,
        expected_version: version,
        idempotency_key: ag_crypto::random_id("resolve")?,
    };
    let thread = client
        .retrying(|| async { client.proposals().resolve_thread(client.req(request.clone())).await })
        .await?
        .into_inner();
    Ok((serde_json::to_value(&thread)?, summary(&thread)))
}
pub async fn run(client: &Client, repo: &str, cmd: &DiscussionCmd) -> Result<(Value, String)> {
    if !client.negotiate().await?.features.iter().any(|f| f == ag_protocol::features::PROPOSAL_THREADS) {
        bail!("this server does not support line discussions; update it before using this command");
    }
    match cmd {
        DiscussionCmd::List { id, page_token } => list(client, repo, id, page_token).await,
        DiscussionCmd::Replies { id, thread, page_token } => replies(client, repo, id, thread, page_token).await,
        DiscussionCmd::Start(args) => start(client, repo, args).await,
        DiscussionCmd::Reply { id, thread, message } => reply(client, repo, id, thread, message).await,
        DiscussionCmd::Resolve { id, thread, expected_version, reopen } => {
            resolve(client, repo, id, thread, *expected_version, *reopen).await
        }
    }
}
