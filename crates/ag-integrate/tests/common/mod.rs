//! Shared fixtures for the engine tests.
#![allow(dead_code, missing_docs)]

use std::sync::Arc;

use ag_integrate::{IntegrateRequest, IntegrationResult, Outcome, Repository};
use ag_storage::ManualClock;
use ag_tree::Tree;
use ag_types::{ChangesetId, FileType, Identity, Namespace, RepoPath, StreamName, Timestamp};

pub struct Fx {
    pub repo: Repository,
    pub clock: Arc<ManualClock>,
}

pub fn id(s: &str) -> Identity {
    Identity::new(s).unwrap()
}

pub fn fx() -> Fx {
    let clock = Arc::new(ManualClock::new(Timestamp::from_millis(1_700_000_000_000)));
    let repo = Repository::ephemeral(Namespace::new("acme/app").unwrap(), StreamName::new("main").unwrap(), clock.clone()).unwrap();
    Fx { repo, clock }
}

impl Fx {
    /// Creates (but does not publish) a changeset whose complete state is `files`.
    pub fn snap(&self, parent: Option<ChangesetId>, files: &[(&str, &str)], author: &str) -> ChangesetId {
        snap_in(&self.repo, parent, files, author)
    }

    pub fn propose(&self, head: ChangesetId, title: &str, author: &str) -> String {
        self.repo.propose("main", &head, title, "", &id(author), None).unwrap().id
    }

    pub fn integrate(&self, prop: &str, actor: &str) -> IntegrationResult {
        self.repo
            .integrate(&IntegrateRequest { proposal_id: prop.into(), actor: id(actor), idempotency_key: None, fencing_tokens: vec![] })
            .unwrap()
    }

    pub fn head(&self) -> Option<ChangesetId> {
        self.repo.meta().read(|tx| tx.stream("main")).unwrap().unwrap().state.head
    }

    pub fn version(&self) -> u64 {
        self.repo.meta().read(|tx| tx.stream("main")).unwrap().unwrap().state.version
    }

    /// Convenience: snapshot + propose + integrate by a human.
    pub fn land(&self, parent: Option<ChangesetId>, files: &[(&str, &str)]) -> ChangesetId {
        let c = self.snap(parent, files, "user:alice");
        let p = self.propose(c, "land", "user:alice");
        let r = self.integrate(&p, "user:alice");
        assert!(matches!(r.outcome, Outcome::Integrated { .. }), "{:?}", r.outcome);
        self.head().unwrap()
    }

    pub fn read(&self, head: &ChangesetId, path: &str) -> Option<String> {
        let t = self.repo.tree_of(head).unwrap();
        t.get(&RepoPath::new(path).unwrap()).map(|e| String::from_utf8(self.repo.blob(&e.content).unwrap()).unwrap())
    }
}

pub fn snap_in(repo: &Repository, parent: Option<ChangesetId>, files: &[(&str, &str)], author: &str) -> ChangesetId {
    let base = repo.tree_or_empty(parent.as_ref()).unwrap();
    let want = Tree::from_entries(
        files.iter().map(|(p, c)| {
            (
                RepoPath::new(*p).unwrap(),
                ag_tree::Entry { content: repo.put_blob(c.as_bytes().to_vec()).unwrap(), file_type: FileType::Regular, executable: false },
            )
        }),
        repo.limits(),
    )
    .unwrap();
    repo.commit(parent.into_iter().collect(), &id(author), "snapshot", base.diff(&want)).unwrap()
}
