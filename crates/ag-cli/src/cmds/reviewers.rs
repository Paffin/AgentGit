//! Requested reviewers: the same inspected-set edit for local and remote proposals.
use std::collections::BTreeSet;

use ag_client::{Client, ClientError};
use ag_protocol::v1;
use ag_types::Identity;
use anyhow::{Result, bail};
use clap::Args;

use crate::ctx::Ctx;
use crate::exit;

/// Inspect review routing, or add/remove people without replacing a concurrent edit.
#[derive(Args, Debug)]
pub struct ReviewersArgs {
    /// Proposal id or unique prefix.
    pub id: String,
    /// Add a requested reviewer; repeat for several identities.
    #[arg(long, value_name = "IDENTITY")]
    pub add: Vec<String>,
    /// Remove a requested reviewer; repeat for several identities.
    #[arg(long, value_name = "IDENTITY")]
    pub remove: Vec<String>,
    /// Require this complete inspected set; repeat for each identity.
    #[arg(long, value_name = "IDENTITY", conflicts_with = "expect_empty")]
    pub expected: Vec<String>,
    /// Require that no reviewers have been requested yet.
    #[arg(long, conflicts_with = "expected")]
    pub expect_empty: bool,
}

struct Edit {
    reviewers: Vec<Identity>,
    expected: Vec<Identity>,
}

/// Review routing names a principal rather than one ephemeral run.
pub fn reviewer_identity(value: &str) -> Result<Identity> {
    Ok(Identity::new(value)?.principal())
}

fn identities(values: &[String]) -> Result<Vec<Identity>> {
    values.iter().map(|value| reviewer_identity(value)).collect()
}

impl ReviewersArgs {
    /// Whether the invocation explicitly requests a change.
    pub fn changes_state(&self) -> bool {
        !self.add.is_empty() || !self.remove.is_empty()
    }

    fn expected(&self, current: &[String]) -> Result<Vec<Identity>> {
        if self.expect_empty {
            return Ok(Vec::new());
        }
        identities(if self.expected.is_empty() { current } else { &self.expected })
    }

    fn edit(&self, current: &[String]) -> Result<Option<Edit>> {
        if !self.changes_state() {
            if self.expect_empty || !self.expected.is_empty() {
                bail!("--expected and --expect-empty require --add or --remove");
            }
            return Ok(None);
        }
        let reviewers = changed_reviewers(current, &self.add, &self.remove)?;
        Ok(Some(Edit { reviewers, expected: self.expected(current)? }))
    }
}

fn changed_reviewers(current: &[String], add: &[String], remove: &[String]) -> Result<Vec<Identity>> {
    let add = identities(add)?;
    let remove = identities(remove)?;
    if add.iter().any(|identity| remove.contains(identity)) {
        bail!("the same reviewer cannot be both added and removed");
    }
    let mut names: BTreeSet<String> = current.iter().cloned().collect();
    for identity in remove {
        names.remove(identity.as_str());
    }
    names.extend(add.into_iter().map(|identity| identity.to_string()));
    identities(&names.into_iter().collect::<Vec<_>>())
}

/// A neutral description: a request never counts as approval or grants access.
pub fn describe(reviewers: &[String]) -> String {
    if reviewers.is_empty() {
        "No reviewers requested.".into()
    } else {
        format!("Requested reviewers:\n  {}", reviewers.join("\n  "))
    }
}

/// Inspect or change the author-owned list in an embedded repository.
pub fn local(ctx: &Ctx, args: &ReviewersArgs) -> Result<u8> {
    let current = ctx.repo.requested_reviewers(&args.id)?;
    if let Some(edit) = args.edit(&current)? {
        let reviewers = ctx.repo.set_requested_reviewers(&args.id, &edit.reviewers, &edit.expected, &ctx.me)?;
        let result = v1::RequestedReviewers { reviewers };
        ctx.done(&result, || describe(&result.reviewers))?;
    } else {
        let result = v1::RequestedReviewers { reviewers: current };
        ctx.out(&result, || describe(&result.reviewers))?;
    }
    Ok(exit::OK)
}

/// Refuse an older server instead of silently treating a requested-reviewer filter as unrestricted.
pub async fn require_support(client: &Client) -> Result<()> {
    if !client.negotiate().await?.features.iter().any(|f| f == ag_protocol::features::PROPOSALS_REVIEW_REQUESTS) {
        bail!("this server does not support requested reviewers; update it before using this command");
    }
    Ok(())
}

/// Use the same edit over gRPC, negotiating support before reading the list.
pub async fn remote(client: &Client, repo: &str, args: &ReviewersArgs) -> Result<v1::RequestedReviewers> {
    require_support(client).await?;
    let detail = client
        .proposals()
        .get(client.req(v1::GetProposalRequest { repo: repo.into(), id: args.id.clone() }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    let Some(edit) = args.edit(&detail.requested_reviewers)? else {
        return Ok(v1::RequestedReviewers { reviewers: detail.requested_reviewers });
    };
    let response = client
        .proposals()
        .set_reviewers(client.req(v1::SetRequestedReviewersRequest {
            repo: repo.into(),
            id: args.id.clone(),
            reviewers: edit.reviewers.into_iter().map(|identity| identity.to_string()).collect(),
            expected_reviewers: edit.expected.into_iter().map(|identity| identity.to_string()).collect(),
        }))
        .await
        .map_err(ClientError::from)?
        .into_inner();
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args() -> ReviewersArgs {
        ReviewersArgs { id: "p".into(), add: vec![], remove: vec![], expected: vec![], expect_empty: false }
    }
    #[test]
    fn edits_keep_unmentioned_reviewers_and_the_inspected_set() {
        let mut args = args();
        args.add = vec!["user:carol".into(), "user:carol".into()];
        let current = vec!["user:bob".into()];
        let edit = args.edit(&current).unwrap().unwrap();
        assert_eq!(edit.reviewers.iter().map(Identity::as_str).collect::<Vec<_>>(), ["user:bob", "user:carol"]);
        assert_eq!(edit.expected.iter().map(Identity::as_str).collect::<Vec<_>>(), ["user:bob"]);
        args.expect_empty = true;
        assert!(args.edit(&current).unwrap().unwrap().expected.is_empty());
    }
    #[test]
    fn removing_a_run_identity_removes_its_canonical_principal() {
        let mut args = args();
        args.remove = vec!["agent:bot#attempt".into()];
        assert!(args.edit(&["agent:bot".into()]).unwrap().unwrap().reviewers.is_empty());
    }
    #[test]
    fn explicit_sets_and_invalid_edits_are_not_silently_ignored() {
        let mut args = args();
        assert!(args.edit(&[]).unwrap().is_none());
        args.expect_empty = true;
        assert!(args.edit(&[]).is_err());
        args.expect_empty = false;
        args.expected = vec!["user:known".into()];
        assert!(args.edit(&[]).is_err());
        args.add = vec!["user:new".into()];
        let edit = args.edit(&[]).unwrap().unwrap();
        assert_eq!(edit.expected[0].as_str(), "user:known");
        args.remove = args.add.clone();
        assert!(args.edit(&[]).is_err());
        args.remove.clear();
        args.add = vec!["not an identity".into()];
        assert!(args.edit(&[]).is_err());
    }
}
