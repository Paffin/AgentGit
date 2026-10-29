//! Explicit cross-repository event replay and resume checkpoints.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ag_client::{Client, ClientError};
use ag_protocol::{features, v1};
use anyhow::{Context, Result, ensure};
use clap::Args;

use crate::Cli;

/// Options which opt the existing event command into a cross-repository feed.
#[derive(Args, Debug)]
pub struct FeedArgs {
    /// Explicit repository on the same server; repeat for up to 30 repositories.
    #[arg(long = "repository", value_name = "OWNER/NAME")]
    pub repos: Vec<String>,
    /// Resume the opaque cursor returned by the previous page.
    #[arg(long, requires = "repos", conflicts_with = "from_start")]
    cursor: Option<String>,
    /// Load a saved cursor, then atomically replace it after emitted events.
    #[arg(long, requires = "repos", value_name = "PATH")]
    checkpoint: Option<PathBuf>,
    /// Deliberately include history; otherwise a new feed starts from now.
    #[arg(long, requires = "repos", conflicts_with = "cursor")]
    from_start: bool,
    /// Maximum events in the replay page (default: 100; maximum: 1000).
    #[arg(long, requires = "repos", value_parser = clap::value_parser!(u32).range(1..=1000))]
    limit: Option<u32>,
}

fn read_cursor(args: &FeedArgs) -> Result<String> {
    if let Some(cursor) = &args.cursor {
        return Ok(cursor.clone());
    }
    if args.from_start {
        return Ok(String::new());
    }
    let Some(path) = &args.checkpoint else {
        return Ok(String::new());
    };
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let mut text = String::new();
    file.take(64 * 1024 + 1).read_to_string(&mut text).with_context(|| format!("reading {}", path.display()))?;
    ensure!(text.len() <= 64 * 1024, "event checkpoint exceeds 64 KiB");
    ensure!(!text.trim().is_empty(), "event checkpoint is empty; remove it to start from now or use --from-start");
    Ok(text.trim().into())
}

fn save_cursor(path: Option<&Path>, cursor: &str) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating checkpoint in {}", parent.display()))?;
    writeln!(file, "{cursor}")?;
    file.as_file().sync_all()?;
    file.persist(path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn print_event(cli: &Cli, envelope: &v1::EventEnvelope) -> Result<()> {
    let Some(event) = &envelope.event else {
        return Ok(());
    };
    let line = if cli.json {
        serde_json::to_string(envelope)?
    } else {
        format!(
            "{}  {:>6}  {}  {}\nCursor: {}",
            event.repo, event.seq, event.topic, event.payload_json, envelope.cursor
        )
    };
    // Flush before advancing the checkpoint so a failed output cannot lose an event.
    crate::ui::print_bytes(format!("{line}\n").as_bytes());
    Ok(())
}

/// Replay one bounded page, optionally following live from that page's cursor.
pub async fn run(
    client: &Client,
    cli: &Cli,
    args: &FeedArgs,
    topics: &[String],
    follow: bool,
    after: u64,
) -> Result<()> {
    ensure!(after == 0, "--after is for a single repository; use --cursor for a cross-repository feed");
    ensure!(args.repos.len() <= 30, "a cross-repository feed accepts at most 30 repositories");
    ensure!(
        client.negotiate().await?.features.iter().any(|feature| feature == features::EVENTS_MULTI_REPO),
        "this server does not support cross-repository events; update it before using --repository"
    );
    let page = client
        .events()
        .replay_many(client.req(v1::ReplayManyRequest {
            repos: args.repos.clone(),
            topics: topics.to_vec(),
            cursor: read_cursor(args)?,
            limit: args.limit.unwrap_or(100),
            from_start: args.from_start,
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    if follow {
        for envelope in &page.events {
            print_event(cli, envelope)?;
            save_cursor(args.checkpoint.as_deref(), &envelope.cursor)?;
        }
    } else if cli.json {
        crate::ui::print_bytes(format!("{}\n", serde_json::to_string(&page)?).as_bytes());
    } else {
        for envelope in &page.events {
            print_event(cli, envelope)?;
        }
        crate::ui::print_bytes(format!("Cursor: {}\nMore: {}\n", page.cursor, page.has_more).as_bytes());
    }
    save_cursor(args.checkpoint.as_deref(), &page.cursor)?;
    if !follow {
        return Ok(());
    }
    let mut stream = client
        .events()
        .subscribe_many(tonic::Request::new(v1::SubscribeManyRequest {
            repos: args.repos.clone(),
            topics: topics.to_vec(),
            cursor: page.cursor,
            heartbeat_ms: 15_000,
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    while let Some(envelope) = stream.message().await.map_err(ClientError::from)? {
        print_event(cli, &envelope)?;
        save_cursor(args.checkpoint.as_deref(), &envelope.cursor)?;
    }
    Ok(())
}
