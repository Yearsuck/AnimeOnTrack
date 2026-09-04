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

fn client() -> reqwest::Client { reqwest::Client::new() }

pub async fn find_backup_file(token: &str) -> Result<Option<String>, String> {
    let resp = client()
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
    let resp = client()
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
    client()
        .patch(format!("{UPLOAD}/{file_id}"))
        .bearer_auth(token)
        .query(&[("uploadType", "media")])
        .body(bytes)
        .send().await.map_err(|e| format!("update: {e}"))?
        .error_for_status().map_err(|e| format!("update status: {e}"))?;
    Ok(())
}

pub async fn get_metadata(token: &str, file_id: &str) -> Result<FileMeta, String> {
    let resp = client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("fields", "size,modifiedTime")])
        .send().await.map_err(|e| format!("meta: {e}"))?
        .error_for_status().map_err(|e| format!("meta status: {e}"))?;
    resp.json().await.map_err(|e| format!("meta json: {e}"))
}

pub async fn download_backup(token: &str, file_id: &str) -> Result<Vec<u8>, String> {
    let resp = client()
        .get(format!("{FILES}/{file_id}"))
        .bearer_auth(token)
        .query(&[("alt", "media")])
        .send().await.map_err(|e| format!("download: {e}"))?
        .error_for_status().map_err(|e| format!("download status: {e}"))?;
    Ok(resp.bytes().await.map_err(|e| format!("download bytes: {e}"))?.to_vec())
}
