use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_LISTEN: &str = "127.0.0.1:8765";
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 30.0;
pub const DEFAULT_MAX_WAIT_TIMEOUT_SECONDS: f64 = 600.0;
pub const DEFAULT_IDLE_FLUSH_SECONDS: f64 = 0.5;
pub const DEFAULT_TERMINAL_COLS: i32 = 200;
pub const DEFAULT_TERMINAL_ROWS: i32 = 50;
pub const DEFAULT_OUTPUT_LIMIT_BYTES: i64 = 16 * 1024;
pub const MAX_OUTPUT_LIMIT_BYTES: i64 = 512 * 1024;
pub const DEFAULT_LIST_LIMIT: i64 = 50;
pub const MAX_LIST_LIMIT: i64 = 200;
pub const DEFAULT_TERMINATE_GRACE_SECONDS: f64 = 10.0;
pub const DEFAULT_GC_INTERVAL_SECONDS: f64 = 60.0;
pub const DEFAULT_GC_FINISHED_JOB_RETENTION_SECONDS: f64 = 300.0;
pub const DEFAULT_POLL_INTERVAL_MS: u64 = 5;
pub const DEFAULT_MAX_POLL_INTERVAL_MS: u64 = 50;
pub const DEFAULT_PIPE_MONITOR_INTERVAL_MS: u64 = 1000;
pub const DEFAULT_PIPE_READY_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_SQLITE_BUSY_TIMEOUT_MS: i32 = 5000;
pub const DEFAULT_AUTH_TOKEN_ENV: &str = "SHELLCTL_AUTH_TOKEN";
pub const HEALTH_STATUS: &str = "ok";
pub const DEFAULT_SNAPSHOT_TIMEOUT_SECONDS: f64 = 45.0;
pub const SNAPSHOT_TIMEOUT_ENV: &str = "SHELLCTL_SNAPSHOT_TIMEOUT";

#[derive(Debug, Clone)]
pub struct Config {
    pub listen: String,
    pub auth_token: String,
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub gc_interval: Duration,
    pub gc_finished_job_retention: Duration,
    pub default_timeout: Duration,
    pub max_wait_timeout: Duration,
    pub idle_flush_duration: Duration,
    pub default_cwd: PathBuf,
    pub default_terminal_cols: i32,
    pub default_terminal_rows: i32,
    pub default_list_limit: i64,
    pub max_list_limit: i64,
    pub default_output_limit_bytes: i64,
    pub max_output_limit_bytes: i64,
    pub default_terminate_grace_seconds: f64,
    pub poll_interval: Duration,
    pub max_poll_interval: Duration,
    pub pipe_monitor_interval: Duration,
    pub pipe_ready_timeout: Duration,
    pub sqlite_busy_timeout_ms: i32,
    pub sanitize_pty_command: Vec<String>,
    pub runner_exit_command: Vec<String>,
    pub snapshot_timeout: Duration,
}

impl Config {
    pub fn jobs_dir(&self) -> PathBuf {
        self.state_dir.join("jobs")
    }
    pub fn db_path(&self) -> PathBuf {
        self.state_dir.join("shellctl.db")
    }
    pub fn tmux_socket(&self) -> PathBuf {
        self.runtime_dir.join("tmux.sock")
    }
    pub fn runner_path(&self) -> PathBuf {
        self.runtime_dir.join("bin").join("shellctl-runner")
    }
}

pub fn default_config() -> Result<Config, String> {
    let home_dir = env::var("HOME").unwrap_or_else(|_| "/home/dify".into());
    let state_dir = default_state_dir(&home_dir);
    let runtime_dir = state_dir.join("runtime");
    let snapshot_timeout = parse_snapshot_timeout(&env::var(SNAPSHOT_TIMEOUT_ENV).unwrap_or_default())?;

    let mut cfg = Config {
        listen: DEFAULT_LISTEN.into(),
        auth_token: String::new(),
        state_dir,
        runtime_dir,
        gc_interval: Duration::from_secs_f64(DEFAULT_GC_INTERVAL_SECONDS),
        gc_finished_job_retention: Duration::from_secs_f64(DEFAULT_GC_FINISHED_JOB_RETENTION_SECONDS),
        default_timeout: Duration::from_secs_f64(DEFAULT_TIMEOUT_SECONDS),
        max_wait_timeout: Duration::from_secs_f64(DEFAULT_MAX_WAIT_TIMEOUT_SECONDS),
        idle_flush_duration: Duration::from_secs_f64(DEFAULT_IDLE_FLUSH_SECONDS),
        default_cwd: PathBuf::from(&home_dir),
        default_terminal_cols: DEFAULT_TERMINAL_COLS,
        default_terminal_rows: DEFAULT_TERMINAL_ROWS,
        default_list_limit: DEFAULT_LIST_LIMIT,
        max_list_limit: MAX_LIST_LIMIT,
        default_output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
        max_output_limit_bytes: MAX_OUTPUT_LIMIT_BYTES,
        default_terminate_grace_seconds: DEFAULT_TERMINATE_GRACE_SECONDS,
        poll_interval: Duration::from_millis(DEFAULT_POLL_INTERVAL_MS),
        max_poll_interval: Duration::from_millis(DEFAULT_MAX_POLL_INTERVAL_MS),
        pipe_monitor_interval: Duration::from_millis(DEFAULT_PIPE_MONITOR_INTERVAL_MS),
        pipe_ready_timeout: Duration::from_millis(DEFAULT_PIPE_READY_TIMEOUT_MS),
        sqlite_busy_timeout_ms: DEFAULT_SQLITE_BUSY_TIMEOUT_MS,
        sanitize_pty_command: vec!["shellctl-sanitize-pty".into()],
        runner_exit_command: vec!["shellctl-runner-exit".into()],
        snapshot_timeout,
    };
    if cfg.auth_token.is_empty() {
        cfg.auth_token = env::var(DEFAULT_AUTH_TOKEN_ENV).unwrap_or_default();
    }
    Ok(cfg)
}

fn default_state_dir(home: &str) -> PathBuf {
    if cfg!(target_os = "macos") {
        return Path::new(home).join(".local/share/shellctl");
    }
    if let Ok(xdg) = env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("shellctl");
        }
    }
    Path::new(home).join(".local/share/shellctl")
}

pub fn parse_snapshot_timeout(raw: &str) -> Result<Duration, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Duration::from_secs_f64(DEFAULT_SNAPSHOT_TIMEOUT_SECONDS));
    }
    let timeout = parse_go_duration(trimmed)
        .map_err(|e| format!("{SNAPSHOT_TIMEOUT_ENV}: {e}"))?;
    if timeout.is_zero() {
        return Err(format!(
            "{SNAPSHOT_TIMEOUT_ENV}: must be positive, got {trimmed:?}"
        ));
    }
    Ok(timeout)
}

/// Minimal Go duration parser: ns, us, µs, ms, s, m, h (optionally concatenated).
pub fn parse_go_duration(s: &str) -> Result<Duration, String> {
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut total = Duration::ZERO;
    let mut saw = false;
    while i < bytes.len() {
        let start = i;
        if bytes[i] == b'-' {
            return Err("must be positive".into());
        }
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
            i += 1;
        }
        if i == start {
            return Err(format!("invalid duration {s:?}"));
        }
        let num: f64 = std::str::from_utf8(&bytes[start..i])
            .unwrap()
            .parse()
            .map_err(|_| format!("invalid duration {s:?}"))?;
        let u0 = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic()
            || (i < bytes.len() && (bytes[i] == 0xC2 || bytes[i] == 0xB5 || bytes[i] == 0xC2))
        {
            // unit letters; µ is utf-8 c2 b5
            if bytes[i].is_ascii_alphabetic() {
                i += 1;
            } else if bytes[i] == 0xC2 && i + 1 < bytes.len() && bytes[i + 1] == 0xB5 {
                i += 2;
            } else {
                break;
            }
        }
        let unit = std::str::from_utf8(&bytes[u0..i]).unwrap_or("");
        let part = match unit {
            "ns" => Duration::from_nanos(num as u64),
            "us" | "µs" | "μs" => Duration::from_nanos((num * 1000.0) as u64),
            "ms" => Duration::from_secs_f64(num / 1000.0),
            "s" => Duration::from_secs_f64(num),
            "m" => Duration::from_secs_f64(num * 60.0),
            "h" => Duration::from_secs_f64(num * 3600.0),
            _ => return Err(format!("invalid duration {s:?}")),
        };
        total += part;
        saw = true;
    }
    if !saw {
        return Err(format!("invalid duration {s:?}"));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_listen() {
        let cfg = default_config().unwrap();
        assert_eq!(cfg.listen, DEFAULT_LISTEN);
        assert_eq!(cfg.default_terminal_cols, DEFAULT_TERMINAL_COLS);
        assert_eq!(cfg.sqlite_busy_timeout_ms, DEFAULT_SQLITE_BUSY_TIMEOUT_MS);
        assert_eq!(cfg.snapshot_timeout, Duration::from_secs_f64(45.0));
    }

    #[test]
    fn paths() {
        let mut cfg = default_config().unwrap();
        cfg.state_dir = PathBuf::from("/tmp/shellctl-test");
        cfg.runtime_dir = PathBuf::from("/tmp/shellctl-test/runtime");
        assert_eq!(cfg.jobs_dir(), PathBuf::from("/tmp/shellctl-test/jobs"));
        assert_eq!(cfg.db_path(), PathBuf::from("/tmp/shellctl-test/shellctl.db"));
        assert_eq!(
            cfg.tmux_socket(),
            PathBuf::from("/tmp/shellctl-test/runtime/tmux.sock")
        );
        assert_eq!(
            cfg.runner_path(),
            PathBuf::from("/tmp/shellctl-test/runtime/bin/shellctl-runner")
        );
    }

    #[test]
    fn snapshot_timeout_parse() {
        assert_eq!(parse_snapshot_timeout("").unwrap(), Duration::from_secs_f64(45.0));
        assert_eq!(parse_snapshot_timeout("10m").unwrap(), Duration::from_secs(600));
        assert!(parse_snapshot_timeout("-1s").is_err());
        assert!(parse_snapshot_timeout("nope").is_err());
    }
}
