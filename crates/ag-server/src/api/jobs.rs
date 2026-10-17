//! Execution plane: validation jobs for trusted workers.

use ag_integrate::Preparation;
use ag_protocol::v1;

use crate::app::{App, Call};
use crate::error::ApiResult;

impl App {
    pub(crate) async fn enqueue_validation_jobs(
        &self,
        _call: &Call,
        _repo: &str,
        _prep: &Preparation,
        _only: &[String],
    ) -> ApiResult<Vec<v1::Job>> {
        Ok(Vec::new())
    }

    /// Queues validation jobs for the required checks of a proposal.
    pub async fn request_validation(
        &self,
        _call: &Call,
        _req: v1::RequestValidationRequest,
    ) -> ApiResult<v1::RequestValidationResponse> {
        Ok(v1::RequestValidationResponse { jobs: Vec::new() })
    }

    /// Claims the next queued job (stub until the job queue lands).
    pub async fn claim_job(&self, _call: &Call, _req: v1::ClaimJobRequest) -> ApiResult<v1::ClaimJobResponse> {
        Ok(v1::ClaimJobResponse { job: None })
    }
    /// Extends a job claim.
    pub async fn heartbeat_job(&self, _call: &Call, _req: v1::HeartbeatJobRequest) -> ApiResult<v1::Job> {
        Err(crate::error::ApiError::new(ag_protocol::ErrorCode::Unimplemented, "jobs are not available yet"))
    }
    /// Completes a job.
    pub async fn complete_job(&self, _call: &Call, _req: v1::CompleteJobRequest) -> ApiResult<v1::Job> {
        Err(crate::error::ApiError::new(ag_protocol::ErrorCode::Unimplemented, "jobs are not available yet"))
    }
    /// Fails a job.
    pub async fn fail_job(&self, _call: &Call, _req: v1::FailJobRequest) -> ApiResult<v1::Job> {
        Err(crate::error::ApiError::new(ag_protocol::ErrorCode::Unimplemented, "jobs are not available yet"))
    }
    /// One job.
    pub async fn get_job(&self, _call: &Call, _req: v1::GetJobRequest) -> ApiResult<v1::Job> {
        Err(crate::error::ApiError::new(ag_protocol::ErrorCode::Unimplemented, "jobs are not available yet"))
    }
    /// Lists jobs.
    pub async fn list_jobs(&self, _call: &Call, _req: v1::ListJobsRequest) -> ApiResult<v1::ListJobsResponse> {
        Ok(v1::ListJobsResponse { jobs: Vec::new() })
    }
    /// The candidate tree of a proposal.
    pub async fn get_candidate(&self, _call: &Call, _req: v1::GetCandidateRequest) -> ApiResult<v1::GetTreeResponse> {
        Err(crate::error::ApiError::new(ag_protocol::ErrorCode::Unimplemented, "jobs are not available yet"))
    }

    pub(crate) async fn existing_validation_jobs(
        &self,
        _call: &Call,
        _repo: &str,
        _prep: &Preparation,
    ) -> ApiResult<Vec<v1::Job>> {
        Ok(Vec::new())
    }
}
