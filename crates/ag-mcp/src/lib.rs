//! `ag-mcp`: the remote AgentGit workflow as a Model Context Protocol server.
//!
//! The server speaks MCP over stdio (newline-delimited JSON-RPC) and turns tool calls into AGWP v1 requests
//! against a running `ag-server`, with a scoped, expiring credential supplied by the operator. See
//! `docs/adr/0008-mcp-server.md` for the decisions and `docs/guides/mcp.md` for how to use it.
//!
//! The protocol layer ([`jsonrpc`], [`protocol`], [`server`]) is independent of AgentGit; the AgentGit-specific
//! parts are [`tools`], [`resources`], [`prompts`] and [`verdicts`]. Everything a tool returns passes through
//! [`output`] and [`sanitize`]: results are data, never instructions.

// Errors here describe a refused or failed tool call: they are built on cold paths, once per call, and carry the
// explanation the agent reads. Boxing them would only add noise to every `?`.
#![allow(clippy::result_large_err)]

pub mod api;
pub mod config;
pub mod jsonrpc;
pub mod log;
pub mod output;
pub mod prompts;
pub mod protocol;
pub mod resources;
pub mod sanitize;
pub mod schema;
pub mod server;
pub mod tools;
pub mod verdicts;

pub use config::{Config, Group, TokenSource};
pub use server::Server;
pub use tools::Registry;
