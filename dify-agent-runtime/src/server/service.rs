use crate::jobmode::Mode;
use crate::server::config::Config;
use crate::server::db::{DB, JobRow, JobStatusName, TransitionOpts};
use crate::server::errors::ServerError;
use crate::server::output::{read_output_window, tail_output_window, OutputWindow};
use crate::server::runtime::{format_timestamp, generate_job_id, parse_timestamp};
use crate::server::tmux::TmuxController;
use crate::server::types::*;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

pub struct Service {
    config: Config,
    db: Option<DB>,
    tmux: TmuxController,
    starting_jobs: Mutex<HashSet<String>>,
    stop_gc: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    stop_mon: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

impl Service {
    pub fn new(config: Config) -> Self {
        let tmux = TmuxController::new(config.clone());
        Self {
            config,
            db: None,
            tmux,
            starting_jobs: Mutex::new(HashSet::new()),
            stop_gc: Mutex::new(None),
            stop_mon: Mutex::new(None),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn initialize(&mut self) -> Result<(), String> {
        self.prepare_runtime()?;
        self.reconcile().map_err(|e| e.to_string())?;
        self.gc_once().map_err(|e| e.to_string())
    }

    pub fn prepare_runtime(&mut self) -> Result<(), String> {
        fs::create_dir_all(&self.config.state_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(&self.config.runtime_dir).map_err(|e| e.to_string())?;
        fs::create_dir_all(self.config.jobs_dir()).map_err(|e| e.to_string())?;
        if let Some(parent) = self.config.runner_path().parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let db = DB::open(&self.config.db_path(), self.config.sqlite_busy_timeout_ms)?;
        db.init_schema()?;
        self.db = Some(db);
        self.install_runner();
        self.tmux.start_server().map_err(|e| e.to_string())
    }

    pub fn shutdown(&self) {
        if let Some(tx) = self.stop_gc.lock().take() {
            let _ = tx.send(());
        }
        if let Some(tx) = self.stop_mon.lock().take() {
            let _ = tx.send(());
        }
    }

    pub fn start_background_gc(self: &Arc<Self>) {
        let (tx, rx) = std::sync::mpsc::channel();
        *self.stop_gc.lock() = Some(tx);
        let svc = Arc::clone(self);
        let interval = self.config.gc_interval;
        thread::spawn(move || loop {
            match rx.recv_timeout(interval) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let _ = svc.gc_once();
                }
            }
        });
    }

    pub fn start_background_pipe_monitor(self: &Arc<Self>) {
        let (tx, rx) = std::sync::mpsc::channel();
        *self.stop_mon.lock() = Some(tx);
        let svc = Arc::clone(self);
        let interval = self.config.pipe_monitor_interval;
        thread::spawn(move || loop {
            match rx.recv_timeout(interval) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    svc.check_running_jobs_pipe_health();
                }
            }
        });
    }

    fn db(&self) -> Result<&DB, ServerError> {
        self.db
            .as_ref()
            .ok_or_else(|| ServerError::new(500, "internal_error", "db not initialized"))
    }

    pub fn run_job(&self, req: &RunJobRequest) -> Result<JobResult, ServerError> {
        eprintln!(
            "RunJob: script={} bytes, cwd={:?}, env_keys={}",
            req.script.len(),
            req.cwd,
            req.env.as_ref().map(|e| e.len()).unwrap_or(0)
        );
        let cwd = self.resolve_cwd(req.cwd.as_deref())?;
        let mut cols = self.config.default_terminal_cols;
        let mut rows = self.config.default_terminal_rows;
        if let Some(t) = &req.terminal {
            cols = t.cols;
            rows = t.rows;
        }
        let mut timeout = self.config.default_timeout;
        if req.timeout > 0.0 {
            timeout = Duration::from_secs_f64(req.timeout);
        }
        let mut output_limit = self.config.default_output_limit_bytes;
        if req.output_limit > 0 {
            output_limit = req.output_limit;
        }
        let mut idle_flush = self.config.idle_flush_duration;
        if req.idle_flush_seconds > 0.0 {
            idle_flush = Duration::from_secs_f64(req.idle_flush_seconds);
        }
        let created_at = format_timestamp(SystemTime::now());
        let (job_id, job_dir) = self.allocate_job_dir()?;
        self.starting_jobs.lock().insert(job_id.clone());

        let script_path = job_dir.join("script");
        let output_path = job_dir.join("output.log");
        let env_path = job_dir.join(".job-env.json");
        if let Err(e) = fs::write(&script_path, req.script.as_bytes()) {
            self.cleanup_starting(&job_id, &job_dir);
            return Err(ServerError::new(500, "internal_error", e.to_string()));
        }
        let env_json = if let Some(env) = &req.env {
            serde_json::to_string(env).unwrap_or_else(|_| "{}".into())
        } else {
            "{}".into()
        };
        if let Err(e) = fs::write(&env_path, env_json) {
            self.cleanup_starting(&job_id, &job_dir);
            return Err(ServerError::new(500, "internal_error", e.to_string()));
        }
        if let Err(e) = fs::write(&output_path, b"") {
            self.cleanup_starting(&job_id, &job_dir);
            return Err(ServerError::new(500, "internal_error", e.to_string()));
        }
        let mode = crate::jobmode::parse(req.mode.as_deref().unwrap_or("")).unwrap_or(Mode::Pty);
        let row = JobRow {
            job_id: job_id.clone(),
            script_path: format!("jobs/{job_id}/script"),
            output_path: format!("jobs/{job_id}/output.log"),
            mode,
            cwd: cwd.to_string_lossy().into_owned(),
            terminal_cols: cols,
            terminal_rows: rows,
            status: JobStatusName::Created,
            session_name: crate::server::runtime::job_session_name(&job_id),
            pane_target: crate::server::runtime::job_pane_target(&job_id),
            exit_code: None,
            reason: None,
            message: None,
            created_at: created_at.clone(),
            started_at: None,
            ended_at: None,
            updated_at: created_at,
        };
        match self.db()?.insert_job(&row) {
            Ok(true) => {}
            Ok(false) => {
                self.cleanup_starting(&job_id, &job_dir);
                return Err(ServerError::new(
                    500,
                    "job_id_collision",
                    "Failed to allocate a unique job id",
                ));
            }
            Err(e) => {
                self.cleanup_starting(&job_id, &job_dir);
                return Err(ServerError::new(500, "internal_error", e));
            }
        }
        let _ = self.db()?.transition_status(
            &job_id,
            TransitionOpts {
                allowed_from: vec![JobStatusName::Created],
                allow_override_from: vec![],
                target: JobStatusName::Starting,
                require_exit_code_null: false,
                reason: None,
                message: None,
                ended_at: String::new(),
            },
        );
        eprintln!("RunJob [{job_id}]: starting job, cwd={}", cwd.display());
        if let Err(start_err) = self.start_job(&job_id, &job_dir, cwd.to_str().unwrap_or("."), cols, rows, mode)
        {
            eprintln!("RunJob [{job_id}]: start failed: {start_err}");
            let _ = self.db()?.transition_status(
                &job_id,
                TransitionOpts {
                    allowed_from: vec![
                        JobStatusName::Created,
                        JobStatusName::Starting,
                        JobStatusName::Running,
                    ],
                    allow_override_from: vec![],
                    target: JobStatusName::Failed,
                    require_exit_code_null: false,
                    reason: Some("start_failed".into()),
                    message: Some(start_err.to_string()),
                    ended_at: String::new(),
                },
            );
            self.tmux.cleanup_session(&job_id);
        } else {
            eprintln!("RunJob [{job_id}]: started successfully");
        }
        self.starting_jobs.lock().remove(&job_id);
        self.wait_job(
            &job_id,
            &WaitJobRequest {
                offset: 0,
                timeout: timeout.as_secs_f64(),
                output_limit,
                idle_flush_seconds: idle_flush.as_secs_f64(),
            },
        )
    }

    fn start_job(
        &self,
        job_id: &str,
        job_dir: &Path,
        cwd: &str,
        cols: i32,
        rows: i32,
        mode: Mode,
    ) -> Result<(), ServerError> {
        eprintln!("startJob [{job_id}]: creating tmux session");
        self.tmux
            .create_job_session(job_id, job_dir, cwd, cols, rows, mode)?;
        let pipe_ready = job_dir.join(".pipe-ready");
        if mode == Mode::Pty {
            eprintln!("startJob [{job_id}]: enabling output pipe");
            self.tmux
                .enable_output_pipe(job_id, job_dir, &pipe_ready)?;
            self.wait_for_pipe_ready(job_id, &pipe_ready)?;
        }
        eprintln!("startJob [{job_id}]: opening start gate");
        fs::write(job_dir.join("start-gate"), b"")
            .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        let _ = self.db()?.transition_status(
            job_id,
            TransitionOpts {
                allowed_from: vec![JobStatusName::Starting],
                allow_override_from: vec![],
                target: JobStatusName::Running,
                require_exit_code_null: true,
                reason: None,
                message: None,
                ended_at: String::new(),
            },
        );
        if mode == Mode::Pty {
            let _ = fs::remove_file(pipe_ready);
        }
        Ok(())
    }

    fn wait_for_pipe_ready(&self, job_id: &str, ready_file: &Path) -> Result<(), ServerError> {
        let deadline = Instant::now() + self.config.pipe_ready_timeout;
        loop {
            if ready_file.exists() {
                match self.tmux.is_output_pipe_active(job_id)? {
                    None => {
                        return Err(ServerError::new(
                            500,
                            "pipe_failed",
                            "tmux pane disappeared after ready-file handshake",
                        ));
                    }
                    Some(true) => return Ok(()),
                    Some(false) => {}
                }
            }
            if Instant::now() > deadline {
                return Err(ServerError::new(
                    500,
                    "pipe_failed",
                    format!(
                        "timed out waiting for pipe ready ({:?})",
                        self.config.pipe_ready_timeout
                    ),
                ));
            }
            thread::sleep(self.config.poll_interval);
        }
    }

    pub fn wait_job(&self, job_id: &str, req: &WaitJobRequest) -> Result<JobResult, ServerError> {
        let row = self.db()?.get_job(job_id)?;
        let output_path = self.output_log_path(&row);
        let timeout = Duration::from_secs_f64(req.timeout);
        let mut output_limit = req.output_limit;
        if output_limit <= 0 {
            output_limit = self.config.default_output_limit_bytes;
        }
        let idle_flush = Duration::from_secs_f64(req.idle_flush_seconds);
        let deadline = Instant::now() + timeout;
        let mut last_size = file_size(&output_path);
        let mut saw_output = last_size > req.offset;
        let mut last_growth: Option<Instant> = if saw_output { Some(Instant::now()) } else { None };
        let mut poll_interval = self.config.poll_interval;
        loop {
            let view = self.get_job_status(job_id)?;
            let current_size = file_size(&output_path);
            if req.offset > current_size {
                return Err(ServerError::new(
                    400,
                    "invalid_offset",
                    format!(
                        "offset {} exceeds current file size {current_size}",
                        req.offset
                    ),
                ));
            }
            if current_size > last_size {
                last_size = current_size;
                if current_size > req.offset {
                    saw_output = true;
                    last_growth = Some(Instant::now());
                }
            }
            if view.done {
                let window = read_output_window(&output_path, req.offset, output_limit)?;
                return Ok(self.job_result_from_view(&view, &row, &window));
            }
            if current_size > req.offset {
                let window = read_output_window(&output_path, req.offset, output_limit)?;
                if window.truncated {
                    return Ok(self.job_result_from_view(&view, &row, &window));
                }
                if saw_output {
                    if let Some(g) = last_growth {
                        if g.elapsed() >= idle_flush {
                            return Ok(self.job_result_from_view(&view, &row, &window));
                        }
                    }
                }
            }
            if Instant::now() > deadline {
                let window = if current_size > req.offset {
                    read_output_window(&output_path, req.offset, output_limit)?
                } else {
                    OutputWindow {
                        output: String::new(),
                        offset: req.offset,
                        truncated: false,
                    }
                };
                return Ok(self.job_result_from_view(&view, &row, &window));
            }
            thread::sleep(poll_interval);
            poll_interval = (poll_interval * 2).min(self.config.max_poll_interval);
        }
    }

    pub fn tail_job(&self, job_id: &str, output_limit: i64) -> Result<JobResult, ServerError> {
        let row = self.db()?.get_job(job_id)?;
        let view = self.get_job_status(job_id)?;
        let window = tail_output_window(&self.output_log_path(&row), output_limit)?;
        Ok(self.job_result_from_view(&view, &row, &window))
    }

    pub fn get_job_status(&self, job_id: &str) -> Result<JobStatusView, ServerError> {
        let row = self.db()?.get_job(job_id)?;
        let (session_exists, pipe_active) = self.live_runtime_state(&row)?;
        self.materialize_status_view(row, session_exists, pipe_active)
    }

    pub fn list_jobs(
        &self,
        status: Option<&str>,
        limit: i64,
    ) -> Result<ListJobsResponse, ServerError> {
        let rows = self.db()?.list_jobs(&[])?;
        let mut items = Vec::new();
        for row in rows {
            let view = match self.get_job_status(&row.job_id) {
                Ok(v) => v,
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            };
            if let Some(s) = status {
                if view.status != s {
                    continue;
                }
            }
            items.push(JobInfo {
                job_id: view.job_id,
                status: view.status,
                created_at: view.created_at,
                started_at: view.started_at,
                ended_at: view.ended_at,
            });
            if items.len() as i64 >= limit {
                break;
            }
        }
        Ok(ListJobsResponse { jobs: items })
    }

    pub fn send_input(&self, job_id: &str, req: &InputJobRequest) -> Result<JobResult, ServerError> {
        let view = self.get_job_status(job_id)?;
        if view.done {
            return Err(ServerError::new(
                409,
                "job_not_running",
                format!("Job {job_id} is already terminal"),
            ));
        }
        let row = self.db()?.get_job(job_id)?;
        if row.mode == Mode::Stdio {
            return Err(ServerError::new(
                409,
                "input_unsupported",
                "stdio jobs do not support input",
            ));
        }
        if let Err(err) = self.tmux.send_input(job_id, &req.text) {
            if err.code == "tmux_target_missing" {
                let view = self.get_job_status(job_id).ok();
                if view.as_ref().map(|v| v.done).unwrap_or(false) {
                    return Err(ServerError::new(
                        409,
                        "job_not_running",
                        format!("Job {job_id} is already terminal"),
                    ));
                }
            }
            return Err(err);
        }
        self.wait_job(
            job_id,
            &WaitJobRequest {
                offset: req.offset,
                timeout: req.timeout,
                output_limit: req.output_limit,
                idle_flush_seconds: req.idle_flush_seconds,
            },
        )
    }

    pub fn terminate_job(
        &self,
        job_id: &str,
        grace_seconds: f64,
    ) -> Result<JobStatusView, ServerError> {
        let view = self.get_job_status(job_id)?;
        if view.done {
            self.tmux.cleanup_session(job_id);
            return Ok(view);
        }
        let _ = self.db()?.transition_status(
            job_id,
            TransitionOpts {
                allowed_from: vec![
                    JobStatusName::Created,
                    JobStatusName::Starting,
                    JobStatusName::Running,
                ],
                allow_override_from: vec![JobStatusName::Exited],
                target: JobStatusName::Terminated,
                require_exit_code_null: false,
                reason: None,
                message: None,
                ended_at: String::new(),
            },
        );
        self.tmux.send_interrupt(job_id);
        if grace_seconds > 0.0 {
            thread::sleep(Duration::from_secs_f64(grace_seconds));
        }
        self.tmux.cleanup_session(job_id);
        self.get_job_status(job_id)
    }

    pub fn delete_job(
        &self,
        job_id: &str,
        force: bool,
        grace_seconds: f64,
    ) -> Result<DeleteJobResponse, ServerError> {
        let view = self.get_job_status(job_id)?;
        if !view.done {
            if !force {
                return Err(ServerError::new(
                    409,
                    "job_running",
                    format!("Job {job_id} is still running"),
                ));
            }
            let _ = self.terminate_job(job_id, grace_seconds);
        }
        self.tmux.cleanup_session(job_id);
        self.db()?.delete_job(job_id)?;
        let _ = fs::remove_dir_all(self.config.jobs_dir().join(job_id));
        Ok(DeleteJobResponse {
            job_id: job_id.into(),
            deleted: true,
        })
    }

    pub fn reconcile(&self) -> Result<(), ServerError> {
        let rows = self.db()?.list_jobs(&[])?;
        for row in rows {
            match self.get_job_status(&row.job_id) {
                Ok(view) => {
                    if view.done {
                        self.tmux.cleanup_session(&row.job_id);
                    }
                }
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn gc_once(&self) -> Result<(), ServerError> {
        let cutoff = SystemTime::now() - self.config.gc_finished_job_retention;
        let rows = self.db()?.list_jobs(&[])?;
        for row in rows {
            let view = match self.get_job_status(&row.job_id) {
                Ok(v) => v,
                Err(e) if e.is_not_found() => continue,
                Err(e) => return Err(e),
            };
            if !view.done || view.ended_at.is_none() {
                continue;
            }
            let Ok(ended) = parse_timestamp(view.ended_at.as_deref().unwrap()) else {
                continue;
            };
            if ended > cutoff {
                continue;
            }
            self.tmux.cleanup_session(&row.job_id);
            let _ = self.db()?.delete_job(&row.job_id);
            let _ = fs::remove_dir_all(self.config.jobs_dir().join(&row.job_id));
        }
        Ok(())
    }

    pub fn check_running_jobs_pipe_health(&self) {
        if let Ok(rows) = self.db().and_then(|d| d.list_jobs(&[JobStatusName::Running])) {
            for row in rows {
                let _ = self.get_job_status(&row.job_id);
            }
        }
    }

    fn materialize_status_view(
        &self,
        mut row: JobRow,
        session_exists: bool,
        pipe_active: Option<bool>,
    ) -> Result<JobStatusView, ServerError> {
        let job_id = row.job_id.clone();
        let status = row.status.clone();
        if status.is_terminal() {
            if row.ended_at.is_none() {
                let now = format_timestamp(SystemTime::now());
                if let Ok(r) = self.db()?.transition_status(
                    &job_id,
                    TransitionOpts {
                        allowed_from: vec![status.clone()],
                        allow_override_from: vec![],
                        target: status.clone(),
                        require_exit_code_null: false,
                        reason: None,
                        message: None,
                        ended_at: now,
                    },
                ) {
                    row = r;
                }
            }
        } else if row.exit_code.is_some() {
            if let Ok(r) = self.db()?.transition_status(
                &job_id,
                TransitionOpts {
                    allowed_from: vec![
                        JobStatusName::Created,
                        JobStatusName::Starting,
                        JobStatusName::Running,
                    ],
                    allow_override_from: vec![],
                    target: JobStatusName::Exited,
                    require_exit_code_null: false,
                    reason: None,
                    message: None,
                    ended_at: String::new(),
                },
            ) {
                row = r;
            }
        } else if let Some(exit) = self.completed_exit_metadata(&row) {
            let _ = self.db()?.record_runner_exit(&job_id, exit.0, &exit.1);
            if let Ok(r) = self.db()?.get_job(&job_id) {
                row = r;
            }
        } else if session_exists {
            if row.mode == Mode::Pty && pipe_active == Some(false) {
                let starting = self.starting_jobs.lock().contains(&job_id);
                if !starting {
                    if let Ok(r) = self.db()?.transition_status(
                        &job_id,
                        TransitionOpts {
                            allowed_from: vec![
                                JobStatusName::Created,
                                JobStatusName::Starting,
                                JobStatusName::Running,
                            ],
                            allow_override_from: vec![],
                            target: JobStatusName::Failed,
                            require_exit_code_null: false,
                            reason: Some("pipe_failed".into()),
                            message: Some(
                                "The tmux output pipe stopped while the job was still running."
                                    .into(),
                            ),
                            ended_at: String::new(),
                        },
                    ) {
                        row = r;
                    }
                }
            } else if matches!(status, JobStatusName::Created | JobStatusName::Starting) {
                let starting = self.starting_jobs.lock().contains(&job_id);
                if !starting {
                    if let Ok(r) = self.db()?.transition_status(
                        &job_id,
                        TransitionOpts {
                            allowed_from: vec![JobStatusName::Created, JobStatusName::Starting],
                            allow_override_from: vec![],
                            target: JobStatusName::Running,
                            require_exit_code_null: true,
                            reason: None,
                            message: None,
                            ended_at: String::new(),
                        },
                    ) {
                        row = r;
                    }
                }
            }
        } else if !self.normal_exit_commit_pending(&row) {
            let starting = self.starting_jobs.lock().contains(&job_id);
            if !starting || (!matches!(status, JobStatusName::Created | JobStatusName::Starting)) {
                if let Ok(r) = self.db()?.transition_status(
                    &job_id,
                    TransitionOpts {
                        allowed_from: vec![
                            JobStatusName::Created,
                            JobStatusName::Starting,
                            JobStatusName::Running,
                        ],
                        allow_override_from: vec![],
                        target: JobStatusName::Lost,
                        require_exit_code_null: false,
                        reason: Some("tmux_session_missing".into()),
                        message: Some("The dedicated tmux session is no longer present.".into()),
                        ended_at: String::new(),
                    },
                ) {
                    row = r;
                }
            }
        }
        Ok(self.status_view_from_row(&row))
    }

    fn live_runtime_state(
        &self,
        row: &JobRow,
    ) -> Result<(bool, Option<bool>), ServerError> {
        let exists = self.tmux.session_exists(&row.session_name)?;
        if !exists {
            return Ok((false, None));
        }
        if row.mode == Mode::Stdio {
            return Ok((true, None));
        }
        match self.tmux.is_output_pipe_active(&row.job_id)? {
            None => Ok((false, None)),
            Some(active) => Ok((true, Some(active))),
        }
    }

    fn completed_exit_metadata(&self, row: &JobRow) -> Option<(i32, String)> {
        let job_dir = self.config.jobs_dir().join(&row.job_id);
        if row.mode == Mode::Pty && !job_dir.join(".pipe-drained").exists() {
            return None;
        }
        let code_p = job_dir.join("runner-exit-code");
        let ended_p = job_dir.join("runner-ended-at");
        if !code_p.exists() || !ended_p.exists() {
            return None;
        }
        let code = fs::read_to_string(code_p).ok()?.trim().to_string();
        let ended = fs::read_to_string(ended_p).ok()?.trim().to_string();
        if code.is_empty() || ended.is_empty() {
            return None;
        }
        let code: i32 = code.parse().ok()?;
        Some((code, ended))
    }

    fn normal_exit_commit_pending(&self, row: &JobRow) -> bool {
        if row.mode != Mode::Pty {
            return false;
        }
        let job_dir = self.config.jobs_dir().join(&row.job_id);
        job_dir.join("runner-exit-code").exists()
            && job_dir.join("runner-ended-at").exists()
            && !job_dir.join(".pipe-drained").exists()
            && !job_dir.join(".pipe-failed").exists()
    }

    fn output_log_path(&self, row: &JobRow) -> PathBuf {
        self.config.state_dir.join(&row.output_path)
    }

    fn status_view_from_row(&self, row: &JobRow) -> JobStatusView {
        let offset = file_size(&self.output_log_path(row));
        JobStatusView {
            job_id: row.job_id.clone(),
            status: row.status.as_str().to_string(),
            done: row.status.is_terminal(),
            exit_code: row.exit_code,
            created_at: row.created_at.clone(),
            started_at: row.started_at.clone(),
            ended_at: row.ended_at.clone(),
            offset,
        }
    }

    fn job_result_from_view(
        &self,
        view: &JobStatusView,
        row: &JobRow,
        window: &OutputWindow,
    ) -> JobResult {
        let abs = fs::canonicalize(self.output_log_path(row))
            .unwrap_or_else(|_| self.output_log_path(row));
        JobResult {
            job_id: view.job_id.clone(),
            done: view.done,
            status: view.status.clone(),
            exit_code: view.exit_code,
            output_path: abs.to_string_lossy().into_owned(),
            output: window.output.clone(),
            offset: window.offset,
            truncated: window.truncated,
        }
    }

    fn resolve_cwd(&self, raw: Option<&str>) -> Result<PathBuf, ServerError> {
        let cwd = match raw {
            Some(s) if !s.is_empty() => PathBuf::from(s),
            _ => self.config.default_cwd.clone(),
        };
        let meta = fs::metadata(&cwd);
        if meta.as_ref().map(|m| m.is_dir()).unwrap_or(false) {
            Ok(fs::canonicalize(&cwd).unwrap_or(cwd))
        } else {
            Err(ServerError::new(
                400,
                "invalid_cwd",
                format!("cwd is not a directory: {}", cwd.display()),
            ))
        }
    }

    fn allocate_job_dir(&self) -> Result<(String, PathBuf), ServerError> {
        for _ in 0..20 {
            let job_id = generate_job_id();
            let job_dir = self.config.jobs_dir().join(&job_id);
            if fs::create_dir(&job_dir).is_ok() {
                return Ok((job_id, job_dir));
            }
        }
        Err(ServerError::new(
            500,
            "job_id_collision",
            "Failed to allocate a unique job id",
        ))
    }

    fn cleanup_starting(&self, job_id: &str, job_dir: &Path) {
        self.starting_jobs.lock().remove(job_id);
        let _ = fs::remove_dir_all(job_dir);
    }

    fn install_runner(&self) {
        let dst = self.config.runner_path();
        let _ = fs::remove_file(&dst);
        let mut src: Option<PathBuf> = None;
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let cand = dir.join("shellctl-runner");
                if cand.exists() {
                    src = Some(cand);
                }
            }
        }
        if src.is_none() {
            if let Ok(p) = which("shellctl-runner") {
                src = Some(p);
            }
        }
        if let Some(src) = src {
            #[cfg(unix)]
            {
                if std::os::unix::fs::symlink(&src, &dst).is_err() {
                    let wrapper = format!("#!/bin/sh\nexec {:?} \"$@\"\n", src);
                    let _ = fs::write(&dst, wrapper);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o755));
                    }
                }
            }
        }
    }
}

fn file_size(path: &Path) -> i64 {
    fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0)
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let p = Path::new(dir).join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(())
}
