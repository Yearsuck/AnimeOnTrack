//! Storage transport for the shared device-sync document.
//!
//! Provides the [`SyncRemote`] trait with an optimistic-locking interface (`get` and
//! conditional `put`), a local filesystem implementation [`LocalFolderRemote`],
//! and the [`run_cycle`] driver that coordinates read, parse, merge and write.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use crate::sync::doc::{parse_doc, to_bytes, SyncDoc, SYNC_FILE_NAME};
use crate::sync::merge::MergeOutput;

#[derive(Debug, Clone, PartialEq)]
pub enum RemoteError {
    /// Someone else wrote the document since our `get` (etag mismatch): get, merge and put again.
    Conflict,
    Other(String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Conflict => write!(f, "conflict: document changed since last read"),
            RemoteError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RemoteError {}

/// Compute lowercase hex SHA-256 etag of bytes.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let hash = Sha256::digest(bytes);
    let mut s = String::with_capacity(64);
    for b in hash {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Where the shared document lives. `etag` is an opaque version token.
#[allow(async_fn_in_trait)]
pub trait SyncRemote {
    /// The document bytes and their etag, or `None` when it does not exist yet.
    async fn get(&self) -> Result<Option<(Vec<u8>, String)>, RemoteError>;
    /// Write the document. `if_match = Some(etag)`: fail with `Conflict` unless the current
    /// etag is still that one. `None`: create; fail with `Conflict` if it already exists.
    /// Returns the new etag.
    async fn put(&self, bytes: &[u8], if_match: Option<&str>) -> Result<String, RemoteError>;
}

pub struct LocalFolderRemote {
    dir: PathBuf,
}

impl LocalFolderRemote {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn file_path(&self) -> PathBuf {
        self.dir.join(SYNC_FILE_NAME)
    }
}

impl SyncRemote for LocalFolderRemote {
    async fn get(&self) -> Result<Option<(Vec<u8>, String)>, RemoteError> {
        let path = self.file_path();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let etag = sha256_hex(&bytes);
                Ok(Some((bytes, etag)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(RemoteError::Other(e.to_string())),
        }
    }

    async fn put(&self, bytes: &[u8], if_match: Option<&str>) -> Result<String, RemoteError> {
        let target_path = self.file_path();
        let current_etag = match std::fs::read(&target_path) {
            Ok(b) => Some(sha256_hex(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(RemoteError::Other(e.to_string())),
        };

        match (if_match, current_etag.as_deref()) {
            (None, Some(_)) => return Err(RemoteError::Conflict),
            (Some(_), None) => return Err(RemoteError::Conflict),
            (Some(expected), Some(actual)) => {
                if expected != actual {
                    return Err(RemoteError::Conflict);
                }
            }
            (None, None) => {}
        }

        if let Err(e) = std::fs::create_dir_all(&self.dir) {
            return Err(RemoteError::Other(e.to_string()));
        }

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let pid = std::process::id();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = self.dir.join(format!(".animeontrack-sync.{pid}.{count}.tmp"));

        if let Err(e) = std::fs::write(&temp_path, bytes) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(RemoteError::Other(e.to_string()));
        }

        // Re-check the etag just before the atomic rename: if another process
        // wrote to (or created) the file between our initial read and now, detect
        // it and return Conflict so the caller retries.
        let recheck = match std::fs::read(&target_path) {
            Ok(b) => Some(sha256_hex(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                let _ = std::fs::remove_file(&temp_path);
                return Err(RemoteError::Other(e.to_string()));
            }
        };
        if recheck.as_deref() != if_match {
            let _ = std::fs::remove_file(&temp_path);
            return Err(RemoteError::Conflict);
        }

        if let Err(e) = std::fs::rename(&temp_path, &target_path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(RemoteError::Other(e.to_string()));
        }

        Ok(sha256_hex(bytes))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncOutcome {
    pub output: MergeOutput,
    pub etag: String,
}

/// get -> parse -> merge -> put, retrying the whole cycle on `Conflict` up to 3 attempts.
/// `merge_fn` receives the parsed remote document and returns the merge output.
pub async fn run_cycle<R: SyncRemote>(
    remote: &R,
    mut merge_fn: impl FnMut(&SyncDoc) -> MergeOutput,
) -> Result<SyncOutcome, String> {
    for attempt in 0..3 {
        let get_res = remote.get().await.map_err(|e| e.to_string())?;

        let (doc, if_match) = match get_res {
            Some((bytes, etag)) => {
                let doc = parse_doc(&bytes).map_err(|e| e.to_string())?;
                (doc, Some(etag))
            }
            None => (SyncDoc::default(), None),
        };

        let output = merge_fn(&doc);
        let bytes = to_bytes(&output.doc);

        match remote.put(&bytes, if_match.as_deref()).await {
            Ok(new_etag) => return Ok(SyncOutcome { output, etag: new_etag }),
            Err(RemoteError::Conflict) => {
                if attempt == 2 {
                    return Err("conflict: sync failed after 3 attempts".to_string());
                }
                continue;
            }
            Err(RemoteError::Other(s)) => return Err(s),
        }
    }
    Err("conflict: sync failed after 3 attempts".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(test_name: &str) -> Self {
            use std::time::SystemTime;
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("aot_test_{pid}_{test_name}_{nanos}"));
            let _ = std::fs::create_dir_all(&path);
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn get_empty_folder_and_put_then_get() {
        let dir = TestDir::new("get_empty");
        let remote = LocalFolderRemote::new(&dir.path);
        tauri::async_runtime::block_on(async {
            let res = remote.get().await.unwrap();
            assert_eq!(res, None);

            let data = b"hello sync bytes";
            let etag = remote.put(data, None).await.unwrap();
            assert_eq!(etag, sha256_hex(data));

            let res = remote.get().await.unwrap();
            assert_eq!(res, Some((data.to_vec(), etag)));
        });
    }

    #[test]
    fn put_with_wrong_etag_conflict() {
        let dir = TestDir::new("wrong_etag");
        let remote = LocalFolderRemote::new(&dir.path);
        tauri::async_runtime::block_on(async {
            let b1 = b"original content";
            let etag1 = remote.put(b1, None).await.unwrap();

            let b2 = b"conflicting content";
            let res = remote
                .put(
                    b2,
                    Some("0000000000000000000000000000000000000000000000000000000000000000"),
                )
                .await;
            assert_eq!(res, Err(RemoteError::Conflict));

            let got = remote.get().await.unwrap();
            assert_eq!(got, Some((b1.to_vec(), etag1)));
        });
    }

    #[test]
    fn put_none_if_match_when_exists_conflict() {
        let dir = TestDir::new("none_conflict");
        let remote = LocalFolderRemote::new(&dir.path);
        tauri::async_runtime::block_on(async {
            let b1 = b"first write";
            let etag1 = remote.put(b1, None).await.unwrap();

            let b2 = b"second write with None";
            let res = remote.put(b2, None).await;
            assert_eq!(res, Err(RemoteError::Conflict));

            let got = remote.get().await.unwrap();
            assert_eq!(got, Some((b1.to_vec(), etag1)));
        });
    }

    #[test]
    fn run_cycle_with_corrupt_file_returns_err_and_file_unchanged() {
        let dir = TestDir::new("corrupt_file");
        let remote = LocalFolderRemote::new(&dir.path);
        tauri::async_runtime::block_on(async {
            let corrupt_bytes = b"not json at all {broken";
            std::fs::write(remote.file_path(), corrupt_bytes).unwrap();

            let res = run_cycle(&remote, |_doc| {
                panic!("merge_fn should not be called on corrupt document");
            })
            .await;

            assert!(res.is_err());
            let on_disk = std::fs::read(remote.file_path()).unwrap();
            assert_eq!(on_disk, corrupt_bytes);
        });
    }

    struct FakeConflictRemote {
        put_count: std::sync::atomic::AtomicUsize,
        conflicts_to_return: usize,
        doc: std::sync::Mutex<Option<(Vec<u8>, String)>>,
    }

    impl FakeConflictRemote {
        fn new(conflicts_to_return: usize) -> Self {
            Self {
                put_count: std::sync::atomic::AtomicUsize::new(0),
                conflicts_to_return,
                doc: std::sync::Mutex::new(None),
            }
        }
    }

    impl SyncRemote for FakeConflictRemote {
        async fn get(&self) -> Result<Option<(Vec<u8>, String)>, RemoteError> {
            let guard = self.doc.lock().unwrap();
            Ok(guard.clone())
        }

        async fn put(&self, bytes: &[u8], _if_match: Option<&str>) -> Result<String, RemoteError> {
            let count = self.put_count.fetch_add(1, Ordering::SeqCst);
            if count < self.conflicts_to_return {
                return Err(RemoteError::Conflict);
            }
            let etag = sha256_hex(bytes);
            let mut guard = self.doc.lock().unwrap();
            *guard = Some((bytes.to_vec(), etag.clone()));
            Ok(etag)
        }
    }

    #[test]
    fn run_cycle_retries_on_conflict_and_succeeds_or_gives_up() {
        // 1. Succeeds on second attempt after 1 conflict
        {
            let fake = FakeConflictRemote::new(1);
            tauri::async_runtime::block_on(async {
                let res = run_cycle(&fake, |_doc| MergeOutput {
                    doc: SyncDoc::default(),
                    apply: Vec::new(),
                    local_changes: 0,
                    remote_changes: 0,
                })
                .await;

                assert!(res.is_ok());
                assert_eq!(fake.put_count.load(Ordering::SeqCst), 2);
            });
        }

        // 2. Fails after 3 conflicts
        {
            let fake = FakeConflictRemote::new(10);
            tauri::async_runtime::block_on(async {
                let res = run_cycle(&fake, |_doc| MergeOutput {
                    doc: SyncDoc::default(),
                    apply: Vec::new(),
                    local_changes: 0,
                    remote_changes: 0,
                })
                .await;

                assert!(res.is_err());
                assert_eq!(fake.put_count.load(Ordering::SeqCst), 3);
            });
        }
    }
}
