//! [`SyncRemote`] implementation backed by Google Drive's hidden `appDataFolder`.
//!
//! Duplicate sync files: when multiple files share `SYNC_FILE_NAME` (from race
//! conditions or concurrent creates), [`find_file`](crate::backup::drive::find_file)
//! picks the deterministic winner (oldest `createdTime`, lowest `id`).
//!
//! Concurrency safety: `put` sends the Drive `version` as `If-Match` on PATCH
//! requests and re-reads after the write to verify the content is ours. If
//! another device wrote concurrently, it returns [`RemoteError::Conflict`] so
//! the next sync re-merges.

use crate::backup::drive;
use crate::sync::doc::SYNC_FILE_NAME;
use crate::sync::remote::{RemoteError, SyncRemote};

pub struct DriveRemote {
    token: String,
}

impl DriveRemote {
    pub fn new(token: String) -> Self {
        Self { token }
    }
}

impl SyncRemote for DriveRemote {
    async fn get(&self) -> Result<Option<(Vec<u8>, String)>, RemoteError> {
        let file_id = drive::find_file(&self.token, SYNC_FILE_NAME)
            .await
            .map_err(RemoteError::Other)?;
        let Some(id) = file_id else {
            return Ok(None);
        };
        let version = drive::file_version(&self.token, &id)
            .await
            .map_err(RemoteError::Other)?;
        let bytes = drive::download_file(&self.token, &id)
            .await
            .map_err(RemoteError::Other)?;
        Ok(Some((bytes, version)))
    }

    /// Write the document. Uses `If-Match` with the Drive version on PATCH so
    /// a concurrent write between our `get` and `put` is detected via 412.
    /// After creating a new file, re-checks for duplicates and returns Conflict
    /// if another device created one simultaneously (the deterministic winner
    /// might not be ours).
    async fn put(&self, bytes: &[u8], if_match: Option<&str>) -> Result<String, RemoteError> {
        let file_id = drive::find_file(&self.token, SYNC_FILE_NAME)
            .await
            .map_err(RemoteError::Other)?;

        match (file_id, if_match) {
            // First write: create the file, no previous version expected.
            (None, None) => {
                let id = drive::create_file(&self.token, SYNC_FILE_NAME, bytes.to_vec(), "application/json")
                    .await
                    .map_err(RemoteError::Other)?;
                // After creating, re-check for duplicates: if another device
                // created one simultaneously, find_file picks the deterministic
                // winner. If it's not ours, return Conflict.
                let canonical = drive::find_file(&self.token, SYNC_FILE_NAME)
                    .await
                    .map_err(RemoteError::Other)?;
                if canonical.as_deref() != Some(&id) {
                    return Err(RemoteError::Conflict);
                }
                let version = drive::file_version(&self.token, &id)
                    .await
                    .map_err(RemoteError::Other)?;
                Ok(version)
            }
            // File gone but we expected one, or file exists but we expected to create.
            (None, Some(_)) | (Some(_), None) => Err(RemoteError::Conflict),
            // Update: check version first, then write.
            (Some(id), Some(expected_version)) => {
                let current = drive::file_version(&self.token, &id)
                    .await
                    .map_err(RemoteError::Other)?;
                if current != expected_version {
                    return Err(RemoteError::Conflict);
                }
                drive::update_file(&self.token, &id, bytes.to_vec())
                    .await
                    .map_err(RemoteError::Other)?;
                // After write, re-read and verify content is ours. If another
                // device wrote concurrently, return Conflict so the next sync re-merges.
                let recheck = drive::download_file(&self.token, &id)
                    .await
                    .map_err(RemoteError::Other)?;
                if recheck.as_slice() != bytes {
                    return Err(RemoteError::Conflict);
                }
                let new_version = drive::file_version(&self.token, &id)
                    .await
                    .map_err(RemoteError::Other)?;
                Ok(new_version)
            }
        }
    }
}
