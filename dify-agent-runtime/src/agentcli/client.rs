use super::env::Environment;
use super::http::HttpClient;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectResponse {
    pub connection_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileDownloadResponse {
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub download_url: String,
}

pub struct StubClient {
    http: HttpClient,
}

pub fn new_stub_client(env: &Environment) -> Result<StubClient, String> {
    let endpoint = super::env::parse_endpoint(&env.url)?;
    let mut normalized = env.clone();
    normalized.url = endpoint.url;
    Ok(StubClient {
        http: HttpClient::new(&normalized),
    })
}

impl StubClient {
    pub async fn connect(&self, argv: &[String], metadata_json: &str) -> Result<ConnectResponse, String> {
        let metadata: Value = serde_json::from_str(metadata_json).unwrap_or_else(|_| json!({}));
        let payload = json!({"argv": argv, "metadata": metadata});
        let (body, status) = self.http.post_json("/connections", &payload).await?;
        check_agent_stub_http_error(&body, status).map_err(|e| format!("agent stub connect failed: {e}"))?;
        serde_json::from_slice(&body).map_err(|e| format!("parse connect response: {e}"))
    }

    pub async fn create_file_upload_url(&self, filename: &str, mimetype: &str) -> Result<String, String> {
        let payload = json!({"filename": filename, "mimetype": mimetype});
        let (body, status) = self.http.post_json("/files/upload-request", &payload).await?;
        check_http_error(&body, status, "file upload request")?;
        parse_upload_url(&body)
    }

    pub async fn create_tool_file_upload_url(
        &self,
        filename: &str,
        mimetype: &str,
    ) -> Result<String, String> {
        let payload = json!({"filename": filename, "mimetype": mimetype});
        let (body, status) = self
            .http
            .post_json("/files/upload-request?expose_expiration=true", &payload)
            .await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub file upload request failed: {e}"))?;
        parse_upload_url(&body)
    }

    pub async fn create_file_download_url(
        &self,
        transfer_method: &str,
        reference: Option<&str>,
        url: Option<&str>,
        for_frontend: bool,
    ) -> Result<FileDownloadResponse, String> {
        let mut file = json!({"transfer_method": transfer_method});
        if let Some(r) = reference {
            file["reference"] = json!(r);
        }
        if let Some(u) = url {
            file["url"] = json!(u);
        }
        let payload = json!({"file": file, "for_frontend": for_frontend});
        let (body, status) = self.http.post_json("/files/download-request", &payload).await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub file download request failed: {e}"))?;
        serde_json::from_slice(&body).map_err(|e| format!("parse download response: {e}"))
    }

    pub async fn get_config_manifest(&self) -> Result<Vec<u8>, String> {
        let (body, status) = self.http.get_json("/config/manifest", &[]).await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub config manifest failed: {e}"))?;
        Ok(body)
    }

    pub async fn create_config_download_url(
        &self,
        kind: &str,
        name: &str,
    ) -> Result<FileDownloadResponse, String> {
        let payload = json!({"config": {"kind": kind, "name": name}, "for_frontend": false});
        let (body, status) = self.http.post_json("/files/download-request", &payload).await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub config download request failed: {e}"))?;
        let resp: FileDownloadResponse =
            serde_json::from_slice(&body).map_err(|e| format!("parse config download response: {e}"))?;
        if resp.download_url.is_empty() {
            return Err("signed config download response is missing download_url".into());
        }
        Ok(resp)
    }

    pub async fn push_config(&self, payload: &Value) -> Result<Vec<u8>, String> {
        let (body, status) = self.http.post_json("/config/push", payload).await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub config push failed: {e}"))?;
        Ok(body)
    }

    pub async fn patch_config_env(&self, env_text: &str) -> Result<Vec<u8>, String> {
        let payload = json!({"env_text": env_text});
        let (body, status) = self.http.patch_json("/config/env", &payload).await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub config env update failed: {e}"))?;
        Ok(body)
    }

    pub async fn put_config_note(&self, note: &str) -> Result<Vec<u8>, String> {
        let payload = json!({"note": note});
        let (body, status) = self.http.put_json("/config/note", &payload).await?;
        check_agent_stub_http_error(&body, status)
            .map_err(|e| format!("agent stub config note update failed: {e}"))?;
        Ok(body)
    }

    pub async fn upload_file_to_url(
        &self,
        upload_url: &str,
        file_path: &str,
        filename: &str,
        mimetype: &str,
    ) -> Result<Vec<u8>, String> {
        self.http
            .upload_file(upload_url, file_path, filename, mimetype)
            .await
    }

    pub async fn download_from_url(&self, download_url: &str) -> Result<Vec<u8>, String> {
        self.http.download_from_url(download_url).await
    }
}

fn parse_upload_url(body: &[u8]) -> Result<String, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("parse upload response: {e}"))?;
    let url = v
        .get("upload_url")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if url.is_empty() {
        return Err("signed file upload response is missing upload_url".into());
    }
    Ok(url.to_string())
}

pub fn check_http_error(body: &[u8], status: u16, operation: &str) -> Result<(), String> {
    if status < 400 {
        return Ok(());
    }
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    if let Some(detail) = v.get("detail") {
        return Err(format!("agent stub {operation} failed (HTTP {status}): {detail}"));
    }
    Err(format!(
        "agent stub {operation} failed (HTTP {status}): {}",
        String::from_utf8_lossy(body)
    ))
}

pub fn check_agent_stub_http_error(body: &[u8], status: u16) -> Result<(), String> {
    if status < 400 {
        return Ok(());
    }
    let mut message = String::from_utf8_lossy(body).into_owned();
    let mut code = String::new();
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        if let Some(detail) = v.get("detail") {
            if let Some(s) = detail.as_str() {
                message = s.to_string();
            } else if let Some(obj) = detail.as_object() {
                if let Some(c) = obj.get("code").and_then(|x| x.as_str()) {
                    code = c.to_string();
                }
                if let Some(m) = obj.get("message").and_then(|x| x.as_str()) {
                    if !m.is_empty() {
                        message = m.to_string();
                    } else {
                        message = detail.to_string();
                    }
                }
            }
        }
    }
    if code == "agent_stub_authorization_expired" {
        return Err(format!(
            "HTTP {status}: Agent Stub authorization expired after 5 minutes; the authorization in this process will not refresh automatically; start a new shell tool call and retry the command"
        ));
    }
    Err(format!("HTTP {status}: {message}"))
}
