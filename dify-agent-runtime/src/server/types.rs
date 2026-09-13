use crate::jobmode::Mode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RunJobRequest {
    pub script: String,
    pub cwd: Option<String>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub terminal: Option<TerminalSize>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub timeout: f64,
    #[serde(default)]
    pub output_limit: i64,
    #[serde(default)]
    pub idle_flush_seconds: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TerminalSize {
    pub cols: i32,
    pub rows: i32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WaitJobRequest {
    #[serde(default)]
    pub timeout: f64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub output_limit: i64,
    #[serde(default)]
    pub idle_flush_seconds: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InputJobRequest {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub timeout: f64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub output_limit: i64,
    #[serde(default)]
    pub idle_flush_seconds: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TerminateJobRequest {
    pub grace_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobResult {
    pub job_id: String,
    pub done: bool,
    pub status: String,
    pub exit_code: Option<i32>,
    pub output_path: String,
    pub output: String,
    pub offset: i64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobStatusView {
    pub job_id: String,
    pub status: String,
    pub done: bool,
    pub exit_code: Option<i32>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobInfo {
    pub job_id: String,
    pub status: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListJobsResponse {
    pub jobs: Vec<JobInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteJobResponse {
    pub job_id: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestoreResponse {
    pub entries: i32,
    pub bytes_written: i64,
}

pub fn resolved_mode(raw: Option<&str>) -> Result<Mode, String> {
    crate::jobmode::parse(raw.unwrap_or(""))
}
