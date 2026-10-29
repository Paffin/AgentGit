//! Private existing-file edits; saving never creates a public object.
use crate::draft_version::{next_version, require_version};
use crate::{Error, Repository, Result};
use ag_storage::meta::FileDraftRow;
use ag_types::{ChangesetId, FileType, Identity, RepoPath, StreamName};

/// Inline draft byte limit, matching browser file edits.
pub const MAX_FILE_DRAFT_BYTES: usize = 4 * 1024 * 1024;
/// Maximum active file drafts per actor in a repository.
pub const MAX_FILE_DRAFTS: u64 = 20;
/// A replacement at the original immutable revision.
pub struct FileDraftInput<'a> {
    /// Branch containing the edited file.
    pub stream: &'a StreamName,
    /// Existing file path.
    pub path: &'a RepoPath,
    /// Original revision; cannot move until the draft is cleared.
    pub base: &'a ChangesetId,
    /// Exact replacement bytes.
    pub content: &'a [u8],
    /// Last observed private draft version.
    pub expected_version: u64,
}
impl Repository {
    /// Reads only this actor's draft, including its clear tombstone.
    pub fn file_draft(&self, owner: &Identity, stream: &StreamName, path: &RepoPath) -> Result<Option<FileDraftRow>> {
        Ok(self.meta.read(|tx| tx.file_draft(owner.as_str(), stream.as_str(), path.as_str()))?)
    }
    /// Lists metadata only for this actor and branch.
    pub fn file_drafts(&self, owner: &Identity, stream: &StreamName) -> Result<Vec<FileDraftRow>> {
        Ok(self.meta.read(|tx| tx.file_drafts(owner.as_str(), stream.as_str()))?)
    }
    /// Saves bytes privately while retaining their validated original revision.
    pub fn save_file_draft(&self, owner: &Identity, input: FileDraftInput<'_>) -> Result<FileDraftRow> {
        if input.content.len() > MAX_FILE_DRAFT_BYTES {
            return Err(Error::Invalid("file draft exceeds 4 MiB".into()));
        }
        // Resolve outside the writer transaction; validate all retained history inside it before saving.
        let original = self
            .tree_of(input.base)?
            .get(input.path)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("file {} at {}", input.path, input.base)))?;
        self.meta.write_with_error(|tx| {
            if tx.stream(input.stream.as_str())?.is_none() {
                return Err(Error::NotFound(format!("stream {}", input.stream)));
            }
            let current = tx.file_draft(owner.as_str(), input.stream.as_str(), input.path.as_str())?;
            if let Some(row) = current.as_ref().filter(|row| row.base.is_some()) {
                if row.base != Some(*input.base) {
                    return Err(Error::Conflict(
                        "file draft retains its original base; clear it before starting another edit".into(),
                    ));
                }
                if row.content == input.content {
                    return Ok(row.clone());
                }
            }
            require_version(current.as_ref().map_or(0, |row| row.version), input.expected_version)?;
            if current.as_ref().is_none_or(|row| row.base.is_none())
                && tx.file_draft_count(owner.as_str())? >= MAX_FILE_DRAFTS
            {
                return Err(Error::Invalid("clear an existing file draft before saving more than 20".into()));
            }
            self.validate_retention_root(tx, input.base)?;
            let row = FileDraftRow {
                stream: input.stream.to_string(),
                path: input.path.to_string(),
                version: next_version(current.as_ref().map_or(0, |row| row.version))?,
                base: Some(*input.base),
                original_blob_id: Some(original.content),
                executable: original.executable,
                file_type: match original.file_type {
                    FileType::Regular => "regular",
                    FileType::Symlink => "symlink",
                }
                .into(),
                content: input.content.to_vec(),
                updated_at: self.clock().now(),
            };
            Ok(tx.replace_file_draft(owner.as_str(), &row)?)
        })
    }
    /// Clears the inspected draft and releases its retained base.
    pub fn clear_file_draft(
        &self,
        owner: &Identity,
        stream: &StreamName,
        path: &RepoPath,
        expected_version: u64,
    ) -> Result<Option<FileDraftRow>> {
        self.meta.write_with_error(|tx| {
            let Some(mut row) = tx.file_draft(owner.as_str(), stream.as_str(), path.as_str())? else { return Ok(None) };
            if row.base.is_none() {
                return Ok(Some(row));
            }
            require_version(row.version, expected_version)?;
            row.version = next_version(row.version)?;
            row.base = None;
            row.original_blob_id = None;
            row.content.clear();
            row.file_type.clear();
            row.executable = false;
            Ok(Some(tx.replace_file_draft(owner.as_str(), &row)?))
        })
    }
}
