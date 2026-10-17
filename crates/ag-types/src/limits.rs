/// Resource limits enforced when objects are decoded, verified and published.
///
/// Limits exist so that a malicious or buggy client cannot make the engine
/// consume unbounded memory or time. All values are upper bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum size of a single blob in bytes.
    pub max_blob_size: u64,
    /// Maximum number of file operations in one changeset.
    pub max_ops: usize,
    /// Maximum number of parents of one changeset.
    pub max_parents: usize,
    /// Maximum length of a changeset message in bytes.
    pub max_message_len: usize,
    /// Maximum generation number (longest chain length) of the DAG.
    pub max_dag_depth: u64,
    /// Maximum number of changesets a single publication may introduce.
    pub max_changesets_per_publication: usize,
    /// Maximum number of entries in a tree.
    pub max_tree_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_blob_size: 256 * 1024 * 1024,
            max_ops: 100_000,
            max_parents: 16,
            max_message_len: 64 * 1024,
            max_dag_depth: 10_000_000,
            max_changesets_per_publication: 10_000,
            max_tree_entries: 2_000_000,
        }
    }
}
