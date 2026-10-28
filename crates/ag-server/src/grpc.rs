//! The gRPC transport.
//!
//! A thin adapter: every RPC builds an [`Ingress`] from the request metadata
//! (`authorization`, `ag-run`, `x-request-id`, `grpc-timeout`), runs the same
//! [`App`] operation the HTTP transport runs, and maps [`ApiError`] to a gRPC
//! status carrying the structured `ErrorInfo` as its detail.

use ag_protocol::v1;
use ag_protocol::v1::{
    admin_service_server::{AdminService, AdminServiceServer},
    agent_service_server::{AgentService, AgentServiceServer},
    event_service_server::{EventService, EventServiceServer},
    execution_service_server::{ExecutionService, ExecutionServiceServer},
    history_service_server::{HistoryService, HistoryServiceServer},
    integration_service_server::{IntegrationService, IntegrationServiceServer},
    issue_service_server::{IssueService, IssueServiceServer},
    object_service_server::{ObjectService, ObjectServiceServer},
    proposal_service_server::{ProposalService, ProposalServiceServer},
    protocol_service_server::{ProtocolService, ProtocolServiceServer},
    repository_service_server::{RepositoryService, RepositoryServiceServer},
    stream_service_server::{StreamService, StreamServiceServer},
    tree_service_server::{TreeService, TreeServiceServer},
    workspace_service_server::{WorkspaceService, WorkspaceServiceServer},
};
use futures_util::{Stream, StreamExt};
use std::pin::Pin;
use tonic::{Request, Response, Status, Streaming};

use crate::app::{App, Ingress};
use crate::error::ApiError;

/// The largest message the server will *send*. Responses are produced by the server (a tree page, a diff, a
/// 1 MiB object chunk), not by the caller, so unlike requests they are not bounded by `limits.max_request_bytes`.
const MAX_RESPONSE_MESSAGE: usize = 16 * 1024 * 1024;

/// Room for the framing around an object chunk's bytes (ids, flags).
const CHUNK_OVERHEAD: usize = 64 * 1024;

/// The largest message the server accepts on every service but the object stream: the same
/// `limits.max_request_bytes` that bounds an HTTP body and that `Negotiate` advertises.
pub fn max_unary_message(cfg: &crate::config::ServerConfig) -> usize {
    usize::try_from(cfg.limits.max_request_bytes).unwrap_or(usize::MAX)
}

/// The largest message the object stream accepts: one chunk always fits, whatever `max_request_bytes` is.
pub fn max_chunk_message(cfg: &crate::config::ServerConfig) -> usize {
    max_unary_message(cfg).max(ag_protocol::limits::CHUNK_BYTES + CHUNK_OVERHEAD)
}

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send + 'static>>;

/// Parses a `grpc-timeout` header value (`<digits><unit>`, units H M S m u n).
pub fn parse_grpc_timeout(v: &str) -> Option<u64> {
    let (digits, unit) = v.split_at(v.len().checked_sub(1)?);
    let n: u64 = digits.parse().ok()?;
    Some(match unit {
        "H" => n.checked_mul(3_600_000)?,
        "M" => n.checked_mul(60_000)?,
        "S" => n.checked_mul(1000)?,
        "m" => n,
        "u" => n / 1000,
        "n" => n / 1_000_000,
        _ => return None,
    })
}

fn ingress_of<T>(req: &Request<T>) -> Ingress {
    let md = req.metadata();
    let get = |k: &str| md.get(k).and_then(|v| v.to_str().ok()).map(str::trim).filter(|s| !s.is_empty());
    let bearer = get("authorization").and_then(|v| {
        let (scheme, token) = v.split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| token.trim().to_string())
    });
    let request_id = get("x-request-id")
        .filter(|id| id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)))
        .map(str::to_string)
        .unwrap_or_else(|| ag_crypto::random_id("req").unwrap_or_else(|_| "req_unknown".into()));
    Ingress {
        transport: "grpc",
        bearer,
        run: get("ag-run").map(str::to_string),
        request_id,
        deadline_ms: get("grpc-timeout").and_then(parse_grpc_timeout),
        traceparent: get("traceparent").map(str::to_string),
        remote: req.remote_addr().map(|a| a.to_string()),
        forwarded: get("forwarded").map(str::to_string),
        x_forwarded_for: get("x-forwarded-for").map(str::to_string),
    }
}

fn status(e: ApiError) -> Status {
    e.0.to_status()
}

/// All gRPC services, backed by one [`App`].
#[derive(Clone)]
pub struct Grpc {
    app: App,
}

macro_rules! unary {
    ($s:ident, $request:ident, $op:literal, $m:ident) => {{
        let ing = ingress_of(&$request);
        let req = $request.into_inner();
        let app = $s.app.clone();
        app.clone()
            .dispatch(ing, $op, move |call| async move { app.$m(&call, req).await })
            .await
            .map(Response::new)
            .map_err(status)
    }};
}

fn chunks(object_id: String, bytes: Vec<u8>) -> BoxStream<v1::ObjectChunk> {
    let total = bytes.len() as u64;
    let size = ag_protocol::limits::CHUNK_BYTES;
    let n = bytes.len().div_ceil(size).max(1);
    let items: Vec<Result<v1::ObjectChunk, Status>> = (0..n)
        .map(|i| {
            let start = i * size;
            let end = ((i + 1) * size).min(bytes.len());
            Ok(v1::ObjectChunk {
                object_id: object_id.clone(),
                total_size: if i == 0 { total } else { 0 },
                data: bytes[start..end].to_vec(),
                last: i + 1 == n,
            })
        })
        .collect();
    Box::pin(futures_util::stream::iter(items))
}

#[tonic::async_trait]
impl ProtocolService for Grpc {
    async fn negotiate(
        &self,
        request: Request<v1::NegotiateRequest>,
    ) -> Result<Response<v1::NegotiateResponse>, Status> {
        self.app.negotiate(request.into_inner()).map(Response::new).map_err(status)
    }
    async fn whoami(&self, request: Request<v1::WhoamiRequest>) -> Result<Response<v1::Principal>, Status> {
        unary!(self, request, "session.whoami", whoami)
    }
}

#[tonic::async_trait]
impl RepositoryService for Grpc {
    async fn create(&self, request: Request<v1::CreateRepoRequest>) -> Result<Response<v1::Repo>, Status> {
        unary!(self, request, "repo.create", create_repo)
    }
    async fn describe(&self, request: Request<v1::GetRepoRequest>) -> Result<Response<v1::Repo>, Status> {
        unary!(self, request, "repo.describe", describe_repo)
    }
    async fn list(&self, request: Request<v1::ListReposRequest>) -> Result<Response<v1::ListReposResponse>, Status> {
        unary!(self, request, "repo.list", list_repos)
    }
    async fn configure(&self, request: Request<v1::ConfigureRepoRequest>) -> Result<Response<v1::Repo>, Status> {
        unary!(self, request, "repo.configure", configure_repo)
    }
}

#[tonic::async_trait]
impl ObjectService for Grpc {
    async fn upload(
        &self,
        request: Request<Streaming<v1::UploadObjectsRequest>>,
    ) -> Result<Response<v1::UploadObjectsResponse>, Status> {
        let ing = ingress_of(&request);
        let mut stream = request.into_inner();
        let app = self.app.clone();
        app.clone()
            .dispatch(ing, "objects.upload", move |call| async move {
                let mut sink: Option<crate::api::ObjectSink> = None;
                loop {
                    let msg = stream
                        .message()
                        .await
                        .map_err(|s| ApiError::invalid(format!("upload stream failed: {}", s.message())))?;
                    let Some(msg) = msg else { break };
                    if sink.is_none() {
                        sink = Some(app.upload_sink(&call, &msg.repo).await?);
                    }
                    if let (Some(s), Some(chunk)) = (sink.as_mut(), msg.chunk) {
                        s.push(chunk).await?;
                    }
                }
                match sink {
                    Some(s) => s.finish(),
                    None => Ok(v1::UploadObjectsResponse::default()),
                }
            })
            .await
            .map(Response::new)
            .map_err(status)
    }

    type GetStream = BoxStream<v1::ObjectChunk>;
    async fn get(&self, request: Request<v1::GetObjectRequest>) -> Result<Response<Self::GetStream>, Status> {
        let ing = ingress_of(&request);
        let req = request.into_inner();
        let (app, id) = (self.app.clone(), req.object_id.clone());
        let bytes = app
            .clone()
            .dispatch(ing, "objects.get", move |call| async move { app.get_object(&call, req).await })
            .await
            .map_err(status)?;
        Ok(Response::new(chunks(id, bytes)))
    }
    async fn missing(
        &self,
        request: Request<v1::MissingObjectsRequest>,
    ) -> Result<Response<v1::MissingObjectsResponse>, Status> {
        unary!(self, request, "objects.missing", missing_objects)
    }
    async fn verify(
        &self,
        request: Request<v1::VerifyObjectsRequest>,
    ) -> Result<Response<v1::VerifyObjectsResponse>, Status> {
        unary!(self, request, "objects.verify", verify_objects)
    }
    async fn publish(&self, request: Request<v1::PublishRequest>) -> Result<Response<v1::Publication>, Status> {
        unary!(self, request, "objects.publish", publish)
    }
    async fn get_operation(
        &self,
        request: Request<v1::GetOperationRequest>,
    ) -> Result<Response<v1::Operation>, Status> {
        unary!(self, request, "operations.get", get_operation)
    }
}

#[tonic::async_trait]
impl HistoryService for Grpc {
    async fn get_changeset(
        &self,
        request: Request<v1::GetChangesetRequest>,
    ) -> Result<Response<v1::Changeset>, Status> {
        unary!(self, request, "history.changeset", get_changeset)
    }
    async fn log(&self, request: Request<v1::LogRequest>) -> Result<Response<v1::LogResponse>, Status> {
        unary!(self, request, "history.log", log)
    }
    async fn blame(&self, request: Request<v1::BlameRequest>) -> Result<Response<v1::BlameResponse>, Status> {
        unary!(self, request, "history.blame", blame)
    }
    async fn ancestors(
        &self,
        request: Request<v1::AncestorsRequest>,
    ) -> Result<Response<v1::AncestorsResponse>, Status> {
        unary!(self, request, "history.ancestors", ancestors)
    }
    async fn merge_base(
        &self,
        request: Request<v1::MergeBaseRequest>,
    ) -> Result<Response<v1::MergeBaseResponse>, Status> {
        unary!(self, request, "history.merge_base", merge_base)
    }
    async fn resolve_rev(
        &self,
        request: Request<v1::ResolveRevRequest>,
    ) -> Result<Response<v1::ResolveRevResponse>, Status> {
        unary!(self, request, "history.resolve", resolve_rev)
    }
}

#[tonic::async_trait]
impl TreeService for Grpc {
    async fn search(
        &self,
        request: Request<v1::SearchTreeRequest>,
    ) -> Result<Response<v1::SearchTreeResponse>, Status> {
        unary!(self, request, "tree.search", search_tree)
    }

    async fn resolve_path(
        &self,
        request: Request<v1::ResolvePathRequest>,
    ) -> Result<Response<v1::ResolvePathResponse>, Status> {
        unary!(self, request, "tree.resolve", resolve_path)
    }
    async fn list_directory(
        &self,
        request: Request<v1::ListDirectoryRequest>,
    ) -> Result<Response<v1::ListDirectoryResponse>, Status> {
        unary!(self, request, "tree.list", list_directory)
    }
    async fn get_tree(&self, request: Request<v1::GetTreeRequest>) -> Result<Response<v1::GetTreeResponse>, Status> {
        unary!(self, request, "tree.get", get_tree)
    }
    async fn diff(&self, request: Request<v1::DiffRequest>) -> Result<Response<v1::DiffResponse>, Status> {
        unary!(self, request, "tree.diff", diff)
    }
    type GetFileStream = BoxStream<v1::ObjectChunk>;
    async fn get_file(&self, request: Request<v1::GetFileRequest>) -> Result<Response<Self::GetFileStream>, Status> {
        let ing = ingress_of(&request);
        let req = request.into_inner();
        let (app, id) = (self.app.clone(), req.path.clone());
        let bytes = app
            .clone()
            .dispatch(ing, "tree.file", move |call| async move { app.file_content(&call, req).await })
            .await
            .map_err(status)?;
        Ok(Response::new(chunks(id, bytes)))
    }
}

#[tonic::async_trait]
impl StreamService for Grpc {
    async fn list(
        &self,
        request: Request<v1::ListStreamsRequest>,
    ) -> Result<Response<v1::ListStreamsResponse>, Status> {
        unary!(self, request, "stream.list", list_streams)
    }
    async fn create(&self, request: Request<v1::CreateStreamRequest>) -> Result<Response<v1::Stream>, Status> {
        unary!(self, request, "stream.create", create_stream)
    }
    async fn read_head(&self, request: Request<v1::ReadHeadRequest>) -> Result<Response<v1::Stream>, Status> {
        unary!(self, request, "stream.head", read_head)
    }
    async fn compare_and_swap(
        &self,
        request: Request<v1::CompareAndSwapRequest>,
    ) -> Result<Response<v1::Stream>, Status> {
        unary!(self, request, "stream.cas", compare_and_swap)
    }
    async fn reflog(&self, request: Request<v1::ReflogRequest>) -> Result<Response<v1::ReflogResponse>, Status> {
        unary!(self, request, "stream.reflog", reflog)
    }
}

#[tonic::async_trait]
impl WorkspaceService for Grpc {
    async fn create(&self, request: Request<v1::CreateWorkspaceRequest>) -> Result<Response<v1::Workspace>, Status> {
        unary!(self, request, "workspace.create", create_workspace)
    }
    async fn get(&self, request: Request<v1::GetWorkspaceRequest>) -> Result<Response<v1::Workspace>, Status> {
        unary!(self, request, "workspace.get", get_workspace)
    }
    async fn list(
        &self,
        request: Request<v1::ListWorkspacesRequest>,
    ) -> Result<Response<v1::ListWorkspacesResponse>, Status> {
        unary!(self, request, "workspace.list", list_workspaces)
    }
    async fn modify(&self, request: Request<v1::ModifyWorkspaceRequest>) -> Result<Response<v1::Workspace>, Status> {
        unary!(self, request, "workspace.modify", modify_workspace)
    }
    async fn sync(
        &self,
        request: Request<v1::SyncWorkspaceRequest>,
    ) -> Result<Response<v1::SyncWorkspaceResponse>, Status> {
        unary!(self, request, "workspace.sync", sync_workspace)
    }
    async fn delete(&self, request: Request<v1::DeleteWorkspaceRequest>) -> Result<Response<v1::Empty>, Status> {
        unary!(self, request, "workspace.delete", delete_workspace)
    }
}

#[tonic::async_trait]
impl ProposalService for Grpc {
    async fn create(&self, request: Request<v1::CreateProposalRequest>) -> Result<Response<v1::Proposal>, Status> {
        unary!(self, request, "proposal.create", create_proposal)
    }
    async fn get(&self, request: Request<v1::GetProposalRequest>) -> Result<Response<v1::ProposalDetail>, Status> {
        unary!(self, request, "proposal.get", get_proposal)
    }
    async fn list(
        &self,
        request: Request<v1::ListProposalsRequest>,
    ) -> Result<Response<v1::ListProposalsResponse>, Status> {
        unary!(self, request, "proposal.list", list_proposals)
    }
    async fn update(&self, request: Request<v1::UpdateProposalRequest>) -> Result<Response<v1::Proposal>, Status> {
        unary!(self, request, "proposal.update", update_proposal)
    }
    async fn review(&self, request: Request<v1::ReviewProposalRequest>) -> Result<Response<v1::Review>, Status> {
        unary!(self, request, "proposal.review", review_proposal)
    }
    async fn comment(&self, request: Request<v1::CommentProposalRequest>) -> Result<Response<v1::Review>, Status> {
        unary!(self, request, "proposal.comment", comment_proposal)
    }
    async fn list_threads(
        &self,
        request: Request<v1::ListReviewThreadsRequest>,
    ) -> Result<Response<v1::ListReviewThreadsResponse>, Status> {
        unary!(self, request, "proposal.threads", list_review_threads)
    }
    async fn create_thread(
        &self,
        request: Request<v1::CreateReviewThreadRequest>,
    ) -> Result<Response<v1::ReviewThread>, Status> {
        unary!(self, request, "proposal.thread_create", create_review_thread)
    }
    async fn list_thread_comments(
        &self,
        request: Request<v1::ListThreadCommentsRequest>,
    ) -> Result<Response<v1::ListThreadCommentsResponse>, Status> {
        unary!(self, request, "proposal.thread_comments", list_thread_comments)
    }
    async fn reply_thread(
        &self,
        request: Request<v1::AddThreadCommentRequest>,
    ) -> Result<Response<v1::ThreadComment>, Status> {
        unary!(self, request, "proposal.thread_reply", reply_review_thread)
    }
    async fn resolve_thread(
        &self,
        request: Request<v1::ResolveReviewThreadRequest>,
    ) -> Result<Response<v1::ReviewThread>, Status> {
        unary!(self, request, "proposal.thread_resolve", resolve_review_thread)
    }
    async fn close(&self, request: Request<v1::CloseProposalRequest>) -> Result<Response<v1::Proposal>, Status> {
        unary!(self, request, "proposal.close", close_proposal)
    }
}

#[tonic::async_trait]
impl IntegrationService for Grpc {
    async fn prepare(&self, request: Request<v1::PrepareRequest>) -> Result<Response<v1::Preparation>, Status> {
        unary!(self, request, "integration.prepare", prepare)
    }
    async fn request_validation(
        &self,
        request: Request<v1::RequestValidationRequest>,
    ) -> Result<Response<v1::RequestValidationResponse>, Status> {
        unary!(self, request, "integration.validate", request_validation)
    }
    async fn submit_attestation(
        &self,
        request: Request<v1::SubmitAttestationRequest>,
    ) -> Result<Response<v1::SubmitAttestationResponse>, Status> {
        unary!(self, request, "integration.attest", submit_attestation)
    }
    async fn list_attestations(
        &self,
        request: Request<v1::ListAttestationsRequest>,
    ) -> Result<Response<v1::ListAttestationsResponse>, Status> {
        unary!(self, request, "integration.attestations", list_attestations)
    }
    async fn integrate(
        &self,
        request: Request<v1::IntegrateRequest>,
    ) -> Result<Response<v1::IntegrationResult>, Status> {
        unary!(self, request, "integration.integrate", integrate)
    }
    async fn enqueue(&self, request: Request<v1::EnqueueRequest>) -> Result<Response<v1::QueueEntry>, Status> {
        unary!(self, request, "queue.enqueue", enqueue)
    }
    async fn list_queue(
        &self,
        request: Request<v1::ListQueueRequest>,
    ) -> Result<Response<v1::ListQueueResponse>, Status> {
        unary!(self, request, "queue.list", list_queue)
    }
    async fn run_queue(&self, request: Request<v1::RunQueueRequest>) -> Result<Response<v1::RunQueueResponse>, Status> {
        unary!(self, request, "queue.run", run_queue)
    }
}

#[tonic::async_trait]
impl AgentService for Grpc {
    async fn register(&self, request: Request<v1::RegisterAgentRequest>) -> Result<Response<v1::Agent>, Status> {
        unary!(self, request, "agent.register", register_agent)
    }
    async fn list(&self, request: Request<v1::ListAgentsRequest>) -> Result<Response<v1::ListAgentsResponse>, Status> {
        unary!(self, request, "agent.list", list_agents)
    }
    async fn get_run(&self, request: Request<v1::GetRunRequest>) -> Result<Response<v1::Run>, Status> {
        unary!(self, request, "agent.get_run", get_run)
    }
    async fn start_run(&self, request: Request<v1::StartRunRequest>) -> Result<Response<v1::Run>, Status> {
        unary!(self, request, "agent.start_run", start_run)
    }
    async fn heartbeat(
        &self,
        request: Request<v1::HeartbeatRequest>,
    ) -> Result<Response<v1::HeartbeatResponse>, Status> {
        unary!(self, request, "agent.heartbeat", heartbeat)
    }
    async fn complete(&self, request: Request<v1::CompleteRunRequest>) -> Result<Response<v1::Run>, Status> {
        unary!(self, request, "agent.complete", complete_run)
    }
    async fn list_runs(&self, request: Request<v1::ListRunsRequest>) -> Result<Response<v1::ListRunsResponse>, Status> {
        unary!(self, request, "agent.list_runs", list_runs)
    }
    async fn reap_runs(&self, request: Request<v1::ReapRunsRequest>) -> Result<Response<v1::ReapRunsResponse>, Status> {
        unary!(self, request, "agent.reap_runs", reap_runs)
    }
    async fn report_executor(
        &self,
        request: Request<v1::ReportExecutorRequest>,
    ) -> Result<Response<v1::ExecutorPresence>, Status> {
        unary!(self, request, "agent.executor.report", report_executor)
    }
    async fn get_run_output(
        &self,
        request: Request<v1::GetRunOutputRequest>,
    ) -> Result<Response<v1::RunOutput>, Status> {
        unary!(self, request, "agent.output.get", get_run_output)
    }
    async fn save_run_output(
        &self,
        request: Request<v1::SaveRunOutputRequest>,
    ) -> Result<Response<v1::RunOutput>, Status> {
        unary!(self, request, "agent.output.save", save_run_output)
    }
    async fn delete_run_output(
        &self,
        request: Request<v1::GetRunOutputRequest>,
    ) -> Result<Response<v1::Empty>, Status> {
        unary!(self, request, "agent.output.delete", delete_run_output)
    }
    async fn get_task_scheduling(
        &self,
        request: Request<v1::GetTaskSchedulingRequest>,
    ) -> Result<Response<v1::TaskScheduling>, Status> {
        unary!(self, request, "task.scheduling.get", get_task_scheduling)
    }
    async fn set_task_scheduling(
        &self,
        request: Request<v1::SetTaskSchedulingRequest>,
    ) -> Result<Response<v1::TaskScheduling>, Status> {
        unary!(self, request, "task.scheduling.set", set_task_scheduling)
    }
    async fn create_task(&self, request: Request<v1::CreateTaskRequest>) -> Result<Response<v1::Task>, Status> {
        unary!(self, request, "task.create", create_task)
    }
    async fn assign_task(&self, request: Request<v1::AssignTaskRequest>) -> Result<Response<v1::Task>, Status> {
        unary!(self, request, "task.assign", assign_task)
    }
    async fn get_task(&self, request: Request<v1::GetTaskRequest>) -> Result<Response<v1::Task>, Status> {
        unary!(self, request, "task.get", get_task)
    }
    async fn list_tasks(
        &self,
        request: Request<v1::ListTasksRequest>,
    ) -> Result<Response<v1::ListTasksResponse>, Status> {
        unary!(self, request, "task.list", list_tasks)
    }
    async fn add_dependency(&self, request: Request<v1::AddDependencyRequest>) -> Result<Response<v1::Task>, Status> {
        unary!(self, request, "task.depend", add_dependency)
    }
    async fn finish_task(&self, request: Request<v1::FinishTaskRequest>) -> Result<Response<v1::Task>, Status> {
        unary!(self, request, "task.finish", finish_task)
    }
    async fn acquire_lease(
        &self,
        request: Request<v1::AcquireLeaseRequest>,
    ) -> Result<Response<v1::AcquireLeaseResponse>, Status> {
        unary!(self, request, "lease.acquire", acquire_lease)
    }
    async fn release_lease(&self, request: Request<v1::LeaseRef>) -> Result<Response<v1::Empty>, Status> {
        unary!(self, request, "lease.release", release_lease)
    }
    async fn extend_lease(&self, request: Request<v1::ExtendLeaseRequest>) -> Result<Response<v1::Lease>, Status> {
        unary!(self, request, "lease.extend", extend_lease)
    }
    async fn list_leases(
        &self,
        request: Request<v1::ListLeasesRequest>,
    ) -> Result<Response<v1::ListLeasesResponse>, Status> {
        unary!(self, request, "lease.list", list_leases)
    }
    async fn submit_provenance(
        &self,
        request: Request<v1::SubmitProvenanceRequest>,
    ) -> Result<Response<v1::ProvenanceEntry>, Status> {
        unary!(self, request, "provenance.submit", submit_provenance)
    }
    async fn list_provenance(
        &self,
        request: Request<v1::ListProvenanceRequest>,
    ) -> Result<Response<v1::ListProvenanceResponse>, Status> {
        unary!(self, request, "provenance.list", list_provenance)
    }
    async fn predict(&self, request: Request<v1::PredictRequest>) -> Result<Response<v1::PredictResponse>, Status> {
        unary!(self, request, "agent.predict", predict)
    }
}

#[tonic::async_trait]
impl EventService for Grpc {
    type SubscribeStream = BoxStream<v1::Event>;
    async fn subscribe(
        &self,
        request: Request<v1::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let mut ing = ingress_of(&request);
        // A subscription is long-lived: the deadline applies to setup only.
        ing.deadline_ms = Some(crate::app::NO_DEADLINE_MS);
        let req = request.into_inner();
        let app = self.app.clone();
        let stream = app
            .clone()
            .dispatch(ing, "events.subscribe", move |call| async move { app.subscribe(call, req).await })
            .await
            .map_err(status)?;
        Ok(Response::new(Box::pin(stream.map(|item| item.map_err(status)))))
    }
    async fn replay(&self, request: Request<v1::ReplayRequest>) -> Result<Response<v1::ReplayResponse>, Status> {
        unary!(self, request, "events.replay", replay)
    }
}

#[tonic::async_trait]
impl AdminService for Grpc {
    async fn get_policy(&self, request: Request<v1::GetPolicyRequest>) -> Result<Response<v1::PolicyVersion>, Status> {
        unary!(self, request, "admin.policy.get", get_policy)
    }
    async fn set_policy(&self, request: Request<v1::SetPolicyRequest>) -> Result<Response<v1::PolicyVersion>, Status> {
        unary!(self, request, "admin.policy.set", set_policy)
    }
    async fn policy_history(
        &self,
        request: Request<v1::PolicyHistoryRequest>,
    ) -> Result<Response<v1::PolicyHistoryResponse>, Status> {
        unary!(self, request, "admin.policy.history", policy_history)
    }
    async fn pin(&self, request: Request<v1::PinRequest>) -> Result<Response<v1::Pin>, Status> {
        unary!(self, request, "admin.pin", pin)
    }
    async fn unpin(&self, request: Request<v1::UnpinRequest>) -> Result<Response<v1::Empty>, Status> {
        unary!(self, request, "admin.unpin", unpin)
    }
    async fn list_pins(&self, request: Request<v1::ListPinsRequest>) -> Result<Response<v1::ListPinsResponse>, Status> {
        unary!(self, request, "admin.pins", list_pins)
    }
    async fn list_audit(
        &self,
        request: Request<v1::ListAuditRequest>,
    ) -> Result<Response<v1::ListAuditResponse>, Status> {
        unary!(self, request, "admin.audit", list_audit)
    }
    async fn verify_audit(
        &self,
        request: Request<v1::VerifyAuditRequest>,
    ) -> Result<Response<v1::VerifyAuditResponse>, Status> {
        unary!(self, request, "admin.audit.verify", verify_audit)
    }
    async fn gc(&self, request: Request<v1::GcRequest>) -> Result<Response<v1::GcResponse>, Status> {
        unary!(self, request, "admin.gc", gc)
    }
    async fn fsck(&self, request: Request<v1::FsckRequest>) -> Result<Response<v1::FsckResponse>, Status> {
        unary!(self, request, "admin.fsck", fsck)
    }
    async fn mint_credential(
        &self,
        request: Request<v1::MintCredentialRequest>,
    ) -> Result<Response<v1::MintCredentialResponse>, Status> {
        unary!(self, request, "admin.credentials.mint", mint_credential)
    }
    async fn list_credentials(
        &self,
        request: Request<v1::ListCredentialsRequest>,
    ) -> Result<Response<v1::ListCredentialsResponse>, Status> {
        unary!(self, request, "admin.credentials.list", list_credentials)
    }
    async fn revoke_credential(
        &self,
        request: Request<v1::RevokeCredentialRequest>,
    ) -> Result<Response<v1::RevokeCredentialResponse>, Status> {
        unary!(self, request, "admin.credentials.revoke", revoke_credential)
    }
    async fn grant_role(&self, request: Request<v1::GrantRoleRequest>) -> Result<Response<v1::RoleBinding>, Status> {
        unary!(self, request, "admin.roles.grant", grant_role)
    }
    async fn revoke_role(&self, request: Request<v1::RevokeRoleRequest>) -> Result<Response<v1::Empty>, Status> {
        unary!(self, request, "admin.roles.revoke", revoke_role)
    }
    async fn list_roles(
        &self,
        request: Request<v1::ListRolesRequest>,
    ) -> Result<Response<v1::ListRolesResponse>, Status> {
        unary!(self, request, "admin.roles.list", list_roles)
    }
}

#[tonic::async_trait]
impl ExecutionService for Grpc {
    async fn claim_job(&self, request: Request<v1::ClaimJobRequest>) -> Result<Response<v1::ClaimJobResponse>, Status> {
        unary!(self, request, "jobs.claim", claim_job)
    }
    async fn heartbeat_job(&self, request: Request<v1::HeartbeatJobRequest>) -> Result<Response<v1::Job>, Status> {
        unary!(self, request, "jobs.heartbeat", heartbeat_job)
    }
    async fn complete_job(&self, request: Request<v1::CompleteJobRequest>) -> Result<Response<v1::Job>, Status> {
        unary!(self, request, "jobs.complete", complete_job)
    }
    async fn fail_job(&self, request: Request<v1::FailJobRequest>) -> Result<Response<v1::Job>, Status> {
        unary!(self, request, "jobs.fail", fail_job)
    }
    async fn get_job(&self, request: Request<v1::GetJobRequest>) -> Result<Response<v1::Job>, Status> {
        unary!(self, request, "jobs.get", get_job)
    }
    async fn list_jobs(&self, request: Request<v1::ListJobsRequest>) -> Result<Response<v1::ListJobsResponse>, Status> {
        unary!(self, request, "jobs.list", list_jobs)
    }
    async fn get_candidate(
        &self,
        request: Request<v1::GetCandidateRequest>,
    ) -> Result<Response<v1::GetTreeResponse>, Status> {
        unary!(self, request, "jobs.candidate", get_candidate)
    }
}

#[tonic::async_trait]
impl IssueService for Grpc {
    async fn create(&self, request: Request<v1::CreateIssueRequest>) -> Result<Response<v1::Issue>, Status> {
        unary!(self, request, "issue.create", create_issue)
    }
    async fn get(&self, request: Request<v1::GetIssueRequest>) -> Result<Response<v1::Issue>, Status> {
        unary!(self, request, "issue.get", get_issue)
    }
    async fn list(&self, request: Request<v1::ListIssuesRequest>) -> Result<Response<v1::ListIssuesResponse>, Status> {
        unary!(self, request, "issue.list", list_issues)
    }
    async fn update(&self, request: Request<v1::UpdateIssueRequest>) -> Result<Response<v1::Issue>, Status> {
        unary!(self, request, "issue.update", update_issue)
    }
    async fn add_comment(
        &self,
        request: Request<v1::AddIssueCommentRequest>,
    ) -> Result<Response<v1::IssueActivity>, Status> {
        unary!(self, request, "issue.comment", add_issue_comment)
    }
    async fn list_activity(
        &self,
        request: Request<v1::ListIssueActivityRequest>,
    ) -> Result<Response<v1::ListIssueActivityResponse>, Status> {
        unary!(self, request, "issue.activity", list_issue_activity)
    }
}

/// Builds the gRPC services as an axum router (so they share the HTTP port).
pub fn router(app: App) -> axum::Router {
    let (unary, chunked) = (max_unary_message(&app.cfg), max_chunk_message(&app.cfg));
    let g = Grpc { app };
    macro_rules! svc {
        ($t:ident) => {
            svc!($t, unary)
        };
        ($t:ident, $decode:expr) => {
            $t::new(g.clone()).max_decoding_message_size($decode).max_encoding_message_size(MAX_RESPONSE_MESSAGE)
        };
    }
    tonic::service::Routes::new(svc!(ProtocolServiceServer))
        .add_service(svc!(RepositoryServiceServer))
        .add_service(svc!(IssueServiceServer))
        .add_service(svc!(ObjectServiceServer, chunked))
        .add_service(svc!(HistoryServiceServer))
        .add_service(svc!(TreeServiceServer))
        .add_service(svc!(StreamServiceServer))
        .add_service(svc!(WorkspaceServiceServer))
        .add_service(svc!(ProposalServiceServer))
        .add_service(svc!(IntegrationServiceServer))
        .add_service(svc!(AgentServiceServer))
        .add_service(svc!(EventServiceServer))
        .add_service(svc!(AdminServiceServer))
        .add_service(svc!(ExecutionServiceServer))
        .into_axum_router()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_timeouts_are_parsed() {
        assert_eq!(parse_grpc_timeout("5S"), Some(5000));
        assert_eq!(parse_grpc_timeout("100m"), Some(100));
        assert_eq!(parse_grpc_timeout("2M"), Some(120_000));
        assert_eq!(parse_grpc_timeout("1H"), Some(3_600_000));
        assert_eq!(parse_grpc_timeout("5000u"), Some(5));
        assert_eq!(parse_grpc_timeout("x"), None);
        assert_eq!(parse_grpc_timeout("5Q"), None);
        assert_eq!(parse_grpc_timeout(""), None);
    }
}
