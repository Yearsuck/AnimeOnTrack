//! Google Drive REST calls scoped to the app's hidden `appDataFolder`.
use serde::Deserialize;

const FILES: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD: &str = "https://www.googleapis.com/upload/drive/v3/files";

#[derive(Deserialize)]
struct FileList { files: Vec<FileRef> }
#[derive(Deserialize)]
struct FileRef { id: String }

#[derive(Deserialize)]
pub struct FileMeta {
    #[serde(default)]
    pub size: Option<String>,
    // Part of Drive's file metadata response; deserialized for completeness
    // but not surfaced in the UI yet.
    #[serde(rename = "modifiedTime", default)]
    #[allow(dead_code)]
    pub modified_time: Option<String>,
}

use std::time::Duration;

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const METADATA_TIMEOUT: Duration = Duration::from_secs(60);
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(600);

fn metadata_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(METADATA_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

fn transfer_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TRANSFER_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

pub async fn find_backup_file(token: &str) -> Result<Option<String>, String> {
    let resp = metadata_client()
        .get(FILES)
        .bearer_auth(token)
        .query(&[
            ("spaces", "appDataFolder"),
            ("q", &format!("name='{}'", super::BACKUP_FILE_NAME)),
            ("fields", "files(id)"),
        ])
        .send().await.map_err(|e| format!("list: {e}"))?
        .error_for_status().map_err(|e| format!("list status: {e}"))?;
    let list: FileList = resp.json().await.map_err(|e| format!("list json: {e}"))?;
    Ok(list.files.into_iter().next().map(|f| f.id))
}

pub async fn create_backup(token: &str, bytes: Vec<u8>) -> Result<String, String> {
    // Multipart upload: metadata part (parents=appDataFolder) + media part.
    let meta = format!(
        r#"{{"name":"{}","parents":["appDataFolder"]}}"#,
        super::BACKUP_FILE_NAME
    );
    let form = reqwest::multipart::Form::new()
        .part(
            "metadata",
            reqwest::multipart::Part::text(meta).mime_str("application/json").unwrap(),
        )
        .part(
            "media",
            reqwest::multipart::Part::bytes(bytes).mime_str("application/octet-stream").unwrap(),
        );
    let resp = transfer_client()
        .post(UPLOAD)
        .bearer_auth(token)
        .query(&[("uploadType", "multipart"), ("fields", "id")])
        .multipart(form)
        .send().await.map_err(|e| format!("create: {e}"))?
        .error_for_status().map_err(|e| format!("create status: {e}"))?;
    let f: FileRef = resp.json().await.map_err(|e| format!("create json: {e}"))?;
    Ok(f.id)
}

pub async fn update_backup(token: &str, file_id: &str, bytes: Vec<u8>) -> Result<(), String> {
    transfer_client()
        .patch(format!("{UPLOAD}/{file_id}"))
        .bearer_auth(token)
        .query(&[("uploadType", "media")])
        .body(bytes)
        .send().await.map_err(|e| format!("update: {e}"))?
        .error_for_status().map_err(|e| format!("update status: {e}"))?;
    Ok(())
}

pub async fn get_metadata(token: &str, file_id: &str) -> Result<FileMeta, String> {
    let resp = metadata_client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("fields", "size,modifiedTime")])
        .send().await.map_err(|e| format!("meta: {e}"))?
        .error_for_status().map_err(|e| format!("meta status: {e}"))?;
    resp.json().await.map_err(|e| format!("meta json: {e}"))
}

pub async fn download_backup(token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let resp = transfer_client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("alt", "media")])
        .send().await.map_err(|e| format!("download: {e}"))?
        .error_for_status().map_err(|e| format!("download status: {e}"))?;
    Ok(resp.bytes().await.map_err(|e| format!("download bytes: {e}"))?.to_vec())
}


/// Choose deterministically when multiple files share the same name:
/// oldest `createdTime` (ascending), tie-break lowest `id` (ascending).
pub fn pick_deterministic_file<I>(files: I) -> Option<String>
where
    I: IntoIterator<Item = (String, Option<String>)>,
{
    files
        .into_iter()
        .min_by(|(id_a, time_a), (id_b, time_b)| {
            match (time_a.as_deref(), time_b.as_deref()) {
                (Some(ta), Some(tb)) => (ta, id_a.as_str()).cmp(&(tb, id_b.as_str())),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => id_a.cmp(id_b),
            }
        })
        .map(|(id, _)| id)
}

/// Look up a file by name in `appDataFolder`, returning its `id` if found.
/// When multiple files share the same name (duplicates from race conditions),
/// chooses deterministically: oldest `createdTime`, tie-break lowest `id`.
pub async fn find_file(token: &str, name: &str) -> Result<Option<String>, String> {
    let resp = metadata_client()
        .get(FILES)
        .bearer_auth(token)
        .query(&[
            ("spaces", "appDataFolder"),
            ("q", &format!("name='{name}'")),
            ("fields", "files(id,createdTime)"),
            ("orderBy", "createdTime"),
        ])
        .send().await.map_err(|e| format!("find_file: {e}"))?
        .error_for_status().map_err(|e| format!("find_file status: {e}"))?;
    #[derive(serde::Deserialize)]
    struct FileWithTime { id: String, #[serde(rename = "createdTime", default)] created_time: Option<String> }
    #[derive(serde::Deserialize)]
    struct List { files: Vec<FileWithTime> }
    let list: List = resp.json().await.map_err(|e| format!("find_file json: {e}"))?;
    Ok(pick_deterministic_file(list.files.into_iter().map(|f| (f.id, f.created_time))))
}

/// Read a file's monotonically-increasing `version` string (Drive v3).
pub async fn file_version(token: &str, file_id: &str) -> Result<String, String> {
    #[derive(Deserialize)]
    struct VersionMeta { version: String }
    let resp = metadata_client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("fields", "version")])
        .send().await.map_err(|e| format!("file_version: {e}"))?
        .error_for_status().map_err(|e| format!("file_version status: {e}"))?;
    let m: VersionMeta = resp.json().await.map_err(|e| format!("file_version json: {e}"))?;
    Ok(m.version)
}

/// Download a file's content. Uses the metadata-timeout client (the sync
/// document is small).
pub async fn download_file(token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let resp = metadata_client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("alt", "media")])
        .send().await.map_err(|e| format!("download_file: {e}"))?
        .error_for_status().map_err(|e| format!("download_file status: {e}"))?;
    Ok(resp.bytes().await.map_err(|e| format!("download_file bytes: {e}"))?.to_vec())
}

/// Create a new file in `appDataFolder` with the given name, bytes and MIME
/// type. Returns the new file's `id`.
pub async fn create_file(token: &str, name: &str, bytes: Vec<u8>, mime: &str) -> Result<String, String> {
    let meta = format!(r#"{{"name":"{name}","parents":["appDataFolder"]}}"#);
    let form = reqwest::multipart::Form::new()
        .part(
            "metadata",
            reqwest::multipart::Part::text(meta).mime_str("application/json").unwrap(),
        )
        .part(
            "media",
            reqwest::multipart::Part::bytes(bytes).mime_str(mime).unwrap(),
        );
    let resp = metadata_client()
        .post(UPLOAD)
        .bearer_auth(token)
        .query(&[("uploadType", "multipart"), ("fields", "id")])
        .multipart(form)
        .send().await.map_err(|e| format!("create_file: {e}"))?
        .error_for_status().map_err(|e| format!("create_file status: {e}"))?;
    let f: FileRef = resp.json().await.map_err(|e| format!("create_file json: {e}"))?;
    Ok(f.id)
}

/// Overwrite an existing file's content.
pub async fn update_file(token: &str, file_id: &str, bytes: Vec<u8>) -> Result<(), String> {
    metadata_client()
        .patch(format!("{UPLOAD}/{file_id}"))
        .bearer_auth(token)
        .query(&[("uploadType", "media")])
        .body(bytes)
        .send().await.map_err(|e| format!("update_file: {e}"))?
        .error_for_status().map_err(|e| format!("update_file status: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_timeouts_are_configured() {
        assert_eq!(CONNECT_TIMEOUT.as_secs(), 15);
        assert_eq!(METADATA_TIMEOUT.as_secs(), 60);
        assert_eq!(TRANSFER_TIMEOUT.as_secs(), 600);
    }

    #[test]
    fn deterministic_file_selection() {
        assert_eq!(pick_deterministic_file(Vec::<(String, Option<String>)>::new()), None);

        let single = vec![("id1".into(), Some("2026-01-01T00:00:00Z".into()))];
        assert_eq!(pick_deterministic_file(single), Some("id1".into()));

        // Oldest createdTime wins
        let files = vec![
            ("id2".into(), Some("2026-01-02T00:00:00Z".into())),
            ("id1".into(), Some("2026-01-01T00:00:00Z".into())),
            ("id3".into(), Some("2026-01-03T00:00:00Z".into())),
        ];
        assert_eq!(pick_deterministic_file(files), Some("id1".into()));

        // Tie in createdTime broken by lowest id lexicographically
        let tie_files = vec![
            ("id_b".into(), Some("2026-01-01T00:00:00Z".into())),
            ("id_a".into(), Some("2026-01-01T00:00:00Z".into())),
            ("id_c".into(), Some("2026-01-01T00:00:00Z".into())),
        ];
        assert_eq!(pick_deterministic_file(tie_files), Some("id_a".into()));

        // File with createdTime wins over one without
        let mixed = vec![
            ("id_no_time".into(), None),
            ("id_with_time".into(), Some("2026-01-01T00:00:00Z".into())),
        ];
        assert_eq!(pick_deterministic_file(mixed), Some("id_with_time".into()));
    }
}
