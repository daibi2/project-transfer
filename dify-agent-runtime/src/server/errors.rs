use std::fmt;

#[derive(Debug, Clone)]
pub struct ServerError {
    pub status_code: u16,
    pub code: String,
    pub message: String,
}

impl ServerError {
    pub fn new(status_code: u16, code: &str, message: impl Into<String>) -> Self {
        Self {
            status_code,
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn job_not_found() -> Self {
        Self::new(404, "job_not_found", "Unknown job id")
    }

    pub fn is_not_found(&self) -> bool {
        self.code == "job_not_found"
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {}: {}",
            self.status_code, self.code, self.message
        )
    }
}

impl std::error::Error for ServerError {}
