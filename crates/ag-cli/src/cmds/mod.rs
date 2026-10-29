//! Command implementations, grouped by area.

pub mod admin;
pub mod agents;
pub mod auth;
pub mod config;
pub mod discussions;
pub mod doctor;
pub mod explain;
pub mod flow;
pub mod gitbridge;
pub mod login;
pub mod overview;
pub mod policy;
pub mod remote;
pub mod repo;
pub mod review_drafts;
pub mod reviewers;
pub mod shell;
pub mod topics;

use crate::ctx::Ctx;

/// A stream chosen positionally (`ag policy show main`) or with `--stream main`.
///
/// `propose`, `policy set`, `workspace create` and the remote commands always took `--stream`; `policy show`,
/// `policy history`, `queue list|run` and `predict` took it as a positional argument. Both spellings are now
/// accepted by all of them, so a script can use one form everywhere. Giving both is a usage error (exit code 2).
#[derive(clap::Args, Debug, Default)]
pub struct StreamSel {
    /// Stream (default: the default stream); `--stream STREAM` means the same.
    #[arg(value_name = "STREAM", conflicts_with = "stream_flag")]
    stream: Option<String>,
    /// The stream, as an option (the same as the positional STREAM).
    #[arg(long = "stream", id = "stream_flag", value_name = "STREAM")]
    stream_flag: Option<String>,
}

impl StreamSel {
    /// The chosen stream, or the repository's default stream.
    pub fn name(&self, ctx: &Ctx) -> String {
        self.stream.clone().or_else(|| self.stream_flag.clone()).unwrap_or_else(|| ctx.default_stream())
    }
}
