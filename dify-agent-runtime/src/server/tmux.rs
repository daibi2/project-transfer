use crate::jobmode::Mode;
use crate::server::config::Config;
use crate::server::errors::ServerError;
use crate::server::runtime::{job_pane_target, job_session_name};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct TmuxController {
    config: Config,
}

impl TmuxController {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn start_server(&self) -> Result<(), ServerError> {
        self.run_tmux(&["start-server", ";", "set-option", "-g", "exit-empty", "off"])?;
        Ok(())
    }

    pub fn list_sessions(&self) -> Result<std::collections::HashSet<String>, ServerError> {
        let result = self.run_tmux_no_check(&["list-sessions", "-F", "#{session_name}"])?;
        if result.exit_code != 0 {
            let stderr = result.stderr.trim();
            if is_tmux_target_missing(stderr) {
                return Ok(std::collections::HashSet::new());
            }
            return Err(ServerError::new(500, "tmux_error", stderr));
        }
        Ok(result
            .stdout
            .trim()
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect())
    }

    pub fn session_exists(&self, session_name: &str) -> Result<bool, ServerError> {
        Ok(self.list_sessions()?.contains(session_name))
    }

    pub fn is_output_pipe_active(&self, job_id: &str) -> Result<Option<bool>, ServerError> {
        let pane = job_pane_target(job_id);
        let result = self.run_tmux_no_check(&[
            "display-message",
            "-p",
            "-t",
            &pane,
            "#{pane_pipe}",
        ])?;
        if result.exit_code != 0 {
            let stderr = result.stderr.trim();
            if is_tmux_target_missing(stderr) {
                return Ok(None);
            }
            return Err(ServerError::new(500, "tmux_error", stderr));
        }
        Ok(Some(result.stdout.trim() == "1"))
    }

    pub fn create_job_session(
        &self,
        job_id: &str,
        job_dir: &Path,
        cwd: &str,
        cols: i32,
        rows: i32,
        mode: Mode,
    ) -> Result<(), ServerError> {
        let runner = self.config.runner_path();
        let runner_cmd = shell_join(&[
            runner.to_string_lossy().as_ref(),
            job_dir.to_string_lossy().as_ref(),
            job_id,
            cwd,
            mode.as_str(),
        ]);
        let cols_s = cols.to_string();
        let rows_s = rows.to_string();
        let session = job_session_name(job_id);
        let result = self.run_tmux_no_check(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            &session,
            "-x",
            &cols_s,
            "-y",
            &rows_s,
            &runner_cmd,
        ])?;
        if result.exit_code != 0 {
            return Err(ServerError::new(
                500,
                "tmux_new_session_failed",
                result.stderr.trim(),
            ));
        }
        Ok(())
    }

    pub fn enable_output_pipe(
        &self,
        job_id: &str,
        job_dir: &Path,
        ready_file: &Path,
    ) -> Result<(), ServerError> {
        let pipe_cmd = self.build_pipe_command(job_id, job_dir, ready_file);
        let pane = job_pane_target(job_id);
        let result = self.run_tmux_no_check(&["pipe-pane", "-o", "-t", &pane, &pipe_cmd])?;
        if result.exit_code != 0 {
            return Err(ServerError::new(500, "pipe_failed", result.stderr.trim()));
        }
        Ok(())
    }

    pub fn send_input(&self, job_id: &str, text: &str) -> Result<(), ServerError> {
        let buffer_name = format!("shellctl-in-{job_id}");
        let tmp_path = self.config.runtime_dir.join(format!(
            "shellctl-input-{job_id}-{}",
            std::process::id()
        ));
        {
            let mut f = std::fs::File::create(&tmp_path)
                .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
            f.write_all(text.as_bytes())
                .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        }
        let tmp_s = tmp_path.to_string_lossy().into_owned();
        let load = self.run_tmux_no_check(&["load-buffer", "-b", &buffer_name, &tmp_s]);
        let _ = std::fs::remove_file(&tmp_path);
        let result = load?;
        if result.exit_code != 0 {
            let stderr = result.stderr.trim();
            if is_tmux_target_missing(stderr) {
                return Err(ServerError::new(409, "tmux_target_missing", stderr));
            }
            return Err(ServerError::new(500, "tmux_input_failed", stderr));
        }
        let pane = job_pane_target(job_id);
        let paste = self.run_tmux_no_check(&["paste-buffer", "-t", &pane, "-b", &buffer_name]);
        let _ = self.run_tmux_no_check(&["delete-buffer", "-b", &buffer_name]);
        let result = paste?;
        if result.exit_code != 0 {
            let stderr = result.stderr.trim();
            if is_tmux_target_missing(stderr) {
                return Err(ServerError::new(409, "tmux_target_missing", stderr));
            }
            return Err(ServerError::new(500, "tmux_input_failed", stderr));
        }
        Ok(())
    }

    pub fn send_interrupt(&self, job_id: &str) {
        let pane = job_pane_target(job_id);
        let _ = self.run_tmux_no_check(&["send-keys", "-t", &pane, "C-c"]);
    }

    pub fn cleanup_session(&self, job_id: &str) {
        let session = job_session_name(job_id);
        let _ = self.run_tmux_no_check(&["kill-session", "-t", &session]);
    }

    fn build_pipe_command(&self, job_id: &str, job_dir: &Path, ready_file: &Path) -> String {
        let mut sanitize_parts = self.config.sanitize_pty_command.clone();
        sanitize_parts.push("--ready-file".into());
        sanitize_parts.push(ready_file.to_string_lossy().into_owned());
        let sanitize_cmd = shell_join(&sanitize_parts.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let busy = self.config.sqlite_busy_timeout_ms.to_string();
        let mut exit_parts = self.config.runner_exit_command.clone();
        exit_parts.extend([
            "--state-dir".into(),
            self.config.state_dir.to_string_lossy().into_owned(),
            "--job-id".into(),
            job_id.into(),
            "--sqlite-busy-timeout-ms".into(),
            busy,
        ]);
        let runner_exit_cmd = shell_join(&exit_parts.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let output_path = shell_quote(&job_dir.join("output.log").to_string_lossy());
        let drained_path = shell_quote(&job_dir.join(".pipe-drained").to_string_lossy());
        let error_log_path = shell_quote(&job_dir.join("pipe-error.log").to_string_lossy());
        let failed_path = shell_quote(&job_dir.join(".pipe-failed").to_string_lossy());
        let exit_code_path = shell_quote(&job_dir.join("runner-exit-code").to_string_lossy());
        let ended_at_path = shell_quote(&job_dir.join("runner-ended-at").to_string_lossy());
        format!(
            "{sanitize_cmd} >> {output_path} 2> {error_log_path} ; sanitize_status=$? ; runner_exit_status=0 ; if [ \"$sanitize_status\" -eq 0 ]; then : > {drained_path}; if [ -s {exit_code_path} ] && [ -s {ended_at_path} ]; then {runner_exit_cmd} --exit-code \"$(cat {exit_code_path})\" --ended-at \"$(cat {ended_at_path})\" 2>> {error_log_path}; runner_exit_status=$?; if [ \"$runner_exit_status\" -ne 0 ]; then printf 'runner-exit failed with status %s\\n' \"$runner_exit_status\" >> {error_log_path}; fi; fi; else : > {failed_path}; fi ; if [ \"$sanitize_status\" -ne 0 ]; then exit \"$sanitize_status\"; fi ; exit \"$sanitize_status\""
        )
    }

    fn run_tmux(&self, args: &[&str]) -> Result<String, ServerError> {
        let result = self.run_tmux_no_check(args)?;
        if result.exit_code != 0 {
            let mut stderr = result.stderr.trim().to_string();
            if stderr.is_empty() {
                stderr = "tmux command failed".into();
            }
            return Err(ServerError::new(500, "tmux_error", stderr));
        }
        Ok(result.stdout)
    }

    fn run_tmux_no_check(&self, args: &[&str]) -> Result<TmuxResult, ServerError> {
        let sock = self.config.tmux_socket();
        let mut cmd = Command::new("tmux");
        cmd.arg("-S").arg(&sock).args(args);
        cmd.env_remove("TMUX");
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        match cmd.output() {
            Ok(out) => Ok(TmuxResult {
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                exit_code: out.status.code().unwrap_or(1),
            }),
            Err(e) => Err(ServerError::new(
                500,
                "tmux_error",
                format!("exec tmux: {e}"),
            )),
        }
    }
}

struct TmuxResult {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn is_tmux_target_missing(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("can't find pane")
        || lower.contains("can't find session")
        || lower.contains("no server running")
        || lower.contains("failed to connect")
        || lower.contains("server exited unexpectedly")
}

fn shell_join(parts: &[&str]) -> String {
    parts.iter().map(|p| shell_quote(p)).collect::<Vec<_>>().join(" ")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
