//! The AgentGit network server.
//!
//! One process serves the AGWP protocol on one port, over two transports
//! that share a single implementation:
//!
//! * **gRPC** (HTTP/2, optionally TLS) for the CLI, SDKs, agents and workers;
//! * **HTTP/JSON + SSE** for browsers and simple clients.
//!
//! Every request flows through the same pipeline: authenticate → rate limit →
//! authorize → run the engine call on a blocking thread → map errors →
//! record metrics. The engine ([`ag_integrate`]) still has the final word on
//! whether a change may land (policy), independent of who is allowed to call
//! which API.

pub mod api;
pub mod app;
pub mod auth;
pub mod authz;
pub mod cluster;
pub mod config;
pub mod control;
pub mod error;
pub mod grpc;
pub mod http;
pub mod jobs_http;
pub mod limits;
pub mod metrics;
pub mod proxy;
pub mod quota;
pub mod repos;
pub mod routes;
pub mod server;
pub mod trace;
pub mod ui;
