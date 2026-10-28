//! Permission-checked saved executor output. Output text is never authoritative evidence.
use super::require;
use crate::{
    app::{App, Call},
    authz::Cap,
    error::{ApiError, ApiResult},
};
use ag_agent::output::{self, Output};
use ag_protocol::v1;

impl App {
    /// Reads the last saved output under the same permission as the run itself.
    pub async fn get_run_output(&self, call: &Call, req: v1::GetRunOutputRequest) -> ApiResult<v1::RunOutput> {
        self.with_repo(call, require(&req.repo, "repo")?, Cap::AgentRead, None, move |repo, _| {
            repo.meta()
                .read(|tx| {
                    if tx.run(&req.run_id)?.is_none() {
                        return Err(ag_storage::meta::MetaError::NotFound("run not found".into()));
                    }
                    Ok(view(&req.run_id, output::read(tx, &req.run_id)?))
                })
                .map_err(Into::into)
        })
        .await
    }

    /// Only operators may publish captured process output.
    pub async fn save_run_output(&self, call: &Call, req: v1::SaveRunOutputRequest) -> ApiResult<v1::RunOutput> {
        let snapshot = req.output.ok_or_else(|| ApiError::invalid("output is required"))?;
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::AgentOperate, None, move |repo, actor| {
            let output = Output {
                sequence: snapshot.sequence,
                stdout: snapshot.stdout,
                stderr: snapshot.stderr,
                stdout_bytes: snapshot.stdout_bytes,
                stderr_bytes: snapshot.stderr_bytes,
                captured_at_ms: 0,
            };
            Ok(view(&req.run_id, Some(output::save(repo.meta(), &req.run_id, &output, actor.as_str())?)))
        })
        .await
    }

    /// Deletes captured output without deleting the run or its audit history.
    pub async fn delete_run_output(&self, call: &Call, req: v1::GetRunOutputRequest) -> ApiResult<v1::Empty> {
        self.with_repo_mut(call, require(&req.repo, "repo")?, Cap::AgentOperate, None, move |repo, actor| {
            output::delete(repo.meta(), &req.run_id, actor.as_str())?;
            Ok(v1::Empty {})
        })
        .await
    }
}

fn view(run_id: &str, output: Option<Output>) -> v1::RunOutput {
    let Some(output) = output else {
        return v1::RunOutput { run_id: run_id.into(), ..Default::default() };
    };
    v1::RunOutput {
        run_id: run_id.into(),
        available: true,
        sequence: output.sequence,
        stdout: output.stdout,
        stderr: output.stderr,
        stdout_bytes: output.stdout_bytes,
        stderr_bytes: output.stderr_bytes,
        captured_at_ms: output.captured_at_ms,
    }
}
