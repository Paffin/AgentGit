//! HTTP routes of the execution plane (jobs).

use std::collections::HashMap;

use ag_protocol::v1;
use axum::Router;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, Method};
use axum::routing::{get, post};

use crate::app::App;
use crate::http::handle;
use crate::routes::RouteMeta;

macro_rules! route {
    ($r:ident, $meta:ident, $method:ident, $verb:literal, $path:literal, $op:literal, $summary:literal, $req:ty, $resp:ty, $m:ident) => {{
        $meta.push(RouteMeta::json::<$req, $resp>($verb, $path, $op, "Execution", $summary));
        $r = $r.route(
            $path,
            $method(
                |State(app): State<App>,
                 method: Method,
                 headers: HeaderMap,
                 params: Option<Path<HashMap<String, String>>>,
                 request: Request| async move {
                    handle::<$req, $resp, _, _>(
                        app,
                        $op,
                        method,
                        headers,
                        params,
                        request,
                        |app, call, req| async move { app.$m(&call, req).await },
                    )
                    .await
                },
            ),
        );
    }};
}

/// Adds the job routes.
pub fn add_routes(r: &mut Router<App>, meta: &mut Vec<RouteMeta>) {
    let mut router = std::mem::take(r);
    route!(
        router,
        meta,
        post,
        "post",
        "/v1/jobs/claim",
        "jobs.claim",
        "Claim the next queued job (long-poll with wait_ms); repo is optional",
        v1::ClaimJobRequest,
        v1::ClaimJobResponse,
        claim_job
    );
    route!(
        router,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/jobs",
        "jobs.list",
        "List jobs",
        v1::ListJobsRequest,
        v1::ListJobsResponse,
        list_jobs
    );
    route!(
        router,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/jobs/{job_id}",
        "jobs.get",
        "One job",
        v1::GetJobRequest,
        v1::Job,
        get_job
    );
    route!(
        router,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/jobs/{job_id}/heartbeat",
        "jobs.heartbeat",
        "Extend the claim on a job",
        v1::HeartbeatJobRequest,
        v1::Job,
        heartbeat_job
    );
    route!(
        router,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/jobs/{job_id}/complete",
        "jobs.complete",
        "Complete a job (with its signed attestation)",
        v1::CompleteJobRequest,
        v1::Job,
        complete_job
    );
    route!(
        router,
        meta,
        post,
        "post",
        "/v1/repos/{owner}/{rname}/jobs/{job_id}/fail",
        "jobs.fail",
        "Fail a job, optionally scheduling a retry",
        v1::FailJobRequest,
        v1::Job,
        fail_job
    );
    route!(
        router,
        meta,
        get,
        "get",
        "/v1/repos/{owner}/{rname}/proposals/{proposal}/candidate",
        "jobs.candidate",
        "The candidate tree of a proposal, for a worker to check out",
        v1::GetCandidateRequest,
        v1::GetTreeResponse,
        get_candidate
    );
    *r = router;
}
