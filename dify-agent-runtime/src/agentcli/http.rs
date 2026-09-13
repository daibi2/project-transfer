use super::env::Environment;
use reqwest::multipart;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_UPLOAD_RESPONSE_BYTES: usize = 1024 * 1024;

pub struct HttpClient {
    base_url: String,
    auth_jwe: String,
    client: reqwest::Client,
}

impl HttpClient {
    pub fn new(env: &Environment) -> Self {
        Self {
            base_url: env.url.clone(),
            auth_jwe: env.auth_jwe.clone(),
            client: reqwest::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .build()
                .expect("client"),
        }
    }

    pub async fn post_json(&self, path: &str, payload: &serde_json::Value) -> Result<(Vec<u8>, u16), String> {
        self.send_json(reqwest::Method::POST, path, Some(payload)).await
    }
    pub async fn patch_json(&self, path: &str, payload: &serde_json::Value) -> Result<(Vec<u8>, u16), String> {
        self.send_json(reqwest::Method::PATCH, path, Some(payload)).await
    }
    pub async fn put_json(&self, path: &str, payload: &serde_json::Value) -> Result<(Vec<u8>, u16), String> {
        self.send_json(reqwest::Method::PUT, path, Some(payload)).await
    }
    pub async fn get_json(&self, path: &str, params: &[(&str, &str)]) -> Result<(Vec<u8>, u16), String> {
        let mut req = self
            .client
            .get(format!("{}{path}", self.base_url))
            .bearer_auth(&self.auth_jwe);
        for (k, v) in params {
            req = req.query(&[(k, v)]);
        }
        let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("read response: {e}"))?;
        Ok((body.to_vec(), status))
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        payload: Option<&serde_json::Value>,
    ) -> Result<(Vec<u8>, u16), String> {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.auth_jwe)
            .header("Content-Type", "application/json");
        if let Some(p) = payload {
            req = req.json(p);
        }
        let resp = req.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("read response: {e}"))?;
        Ok((body.to_vec(), status))
    }

    pub async fn upload_file(
        &self,
        upload_url: &str,
        file_path: &str,
        filename: &str,
        mimetype: &str,
    ) -> Result<Vec<u8>, String> {
        let bytes = std::fs::read(file_path).map_err(|e| format!("open file: {e}"))?;
        let part = multipart::Part::bytes(bytes)
            .file_name(filename.to_string())
            .mime_str(mimetype)
            .unwrap_or_else(|_| multipart::Part::bytes(Vec::new()));
        let form = multipart::Form::new().part("file", part);
        let client = reqwest::Client::builder()
            .timeout(UPLOAD_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .post(upload_url)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "upload request timed out".to_string()
                } else {
                    "upload request failed".to_string()
                }
            })?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(|e| format!("read upload response: {e}"))?;
        if !status.is_success() {
            let mut b = body.to_vec();
            if b.len() > MAX_UPLOAD_RESPONSE_BYTES {
                b.truncate(MAX_UPLOAD_RESPONSE_BYTES);
            }
            return Err(format!(
                "upload failed with status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&b)
            ));
        }
        if body.len() > MAX_UPLOAD_RESPONSE_BYTES {
            return Err(format!(
                "upload response exceeds {MAX_UPLOAD_RESPONSE_BYTES} bytes"
            ));
        }
        Ok(body.to_vec())
    }

    pub async fn download_from_url(&self, download_url: &str) -> Result<Vec<u8>, String> {
        let client = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?;
        let resp = client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("download request failed: {e}"))?;
        let status = resp.status();
        let body = resp.bytes().await.map_err(|e| e.to_string())?;
        if status.as_u16() >= 400 {
            return Err(format!(
                "download failed with status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            ));
        }
        Ok(body.to_vec())
    }
}
