use crate::jobmode::Mode;
use crate::server::errors::ServerError;
use crate::server::runtime::format_timestamp;
use parking_lot::Mutex;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};
use std::path::Path;
use std::time::Duration;

const LATEST_SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatusName {
    Created,
    Starting,
    Running,
    Exited,
    Terminated,
    Failed,
    Lost,
    Other(String),
}

impl JobStatusName {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Created => "created",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Terminated => "terminated",
            Self::Failed => "failed",
            Self::Lost => "lost",
            Self::Other(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "created" => Self::Created,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "exited" => Self::Exited,
            "terminated" => Self::Terminated,
            "failed" => Self::Failed,
            "lost" => Self::Lost,
            other => Self::Other(other.into()),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Exited | Self::Terminated | Self::Failed | Self::Lost
        )
    }
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub job_id: String,
    pub script_path: String,
    pub output_path: String,
    pub mode: Mode,
    pub cwd: String,
    pub terminal_cols: i32,
    pub terminal_rows: i32,
    pub status: JobStatusName,
    pub session_name: String,
    pub pane_target: String,
    pub exit_code: Option<i32>,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub updated_at: String,
}

pub struct TransitionOpts {
    pub allowed_from: Vec<JobStatusName>,
    pub allow_override_from: Vec<JobStatusName>,
    pub target: JobStatusName,
    pub require_exit_code_null: bool,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub ended_at: String,
}

pub struct DB {
    conn: Mutex<Connection>,
}

impl DB {
    pub fn open(db_path: &Path, busy_timeout_ms: i32) -> Result<Self, String> {
        let conn = Connection::open(db_path).map_err(|e| format!("open sqlite: {e}"))?;
        conn.busy_timeout(Duration::from_millis(busy_timeout_ms as u64))
            .map_err(|e| e.to_string())?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn init_schema(&self) -> Result<(), String> {
        let conn = self.conn.lock();
        let mut current: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| format!("read schema version: {e}"))?;
        if current > LATEST_SCHEMA_VERSION {
            return Err(format!(
                "database schema version {current} is newer than supported version {LATEST_SCHEMA_VERSION}"
            ));
        }
        if current == 0 {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS jobs (
			job_id       TEXT PRIMARY KEY,
			script_path  TEXT NOT NULL,
			output_path  TEXT NOT NULL,
			cwd          TEXT NOT NULL,
			terminal_cols INTEGER NOT NULL DEFAULT 200,
			terminal_rows INTEGER NOT NULL DEFAULT 50,
			status       TEXT NOT NULL DEFAULT 'created',
			session_name TEXT NOT NULL,
			pane_target  TEXT NOT NULL,
			exit_code    INTEGER,
			reason       TEXT,
			message      TEXT,
			created_at   TEXT NOT NULL,
			started_at   TEXT,
			ended_at     TEXT,
			updated_at   TEXT NOT NULL
		)",
            )
            .map_err(|e| format!("create schema v0: {e}"))?;
        }
        while current < LATEST_SCHEMA_VERSION {
            let target = current + 1;
            if target == 1 {
                conn.execute(
                    "ALTER TABLE jobs ADD COLUMN mode TEXT NOT NULL DEFAULT 'pty'",
                    [],
                )
                .map_err(|e| format!("apply schema migration v1: {e}"))?;
            }
            conn.pragma_update(None, "user_version", target)
                .map_err(|e| format!("record schema migration v{target}: {e}"))?;
            current = target;
        }
        Ok(())
    }

    pub fn insert_job(&self, row: &JobRow) -> Result<bool, String> {
        let conn = self.conn.lock();
        let res = conn.execute(
            "INSERT INTO jobs (job_id, script_path, output_path, mode, cwd, terminal_cols, terminal_rows,
			status, session_name, pane_target, exit_code, reason, message,
			created_at, started_at, ended_at, updated_at)
		VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                row.job_id,
                row.script_path,
                row.output_path,
                row.mode.as_str(),
                row.cwd,
                row.terminal_cols,
                row.terminal_rows,
                row.status.as_str(),
                row.session_name,
                row.pane_target,
                row.exit_code,
                row.reason,
                row.message,
                row.created_at,
                row.started_at,
                row.ended_at,
                row.updated_at,
            ],
        );
        match res {
            Ok(_) => Ok(true),
            Err(e) if e.to_string().contains("UNIQUE constraint failed") => Ok(false),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn get_job(&self, job_id: &str) -> Result<JobRow, ServerError> {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT job_id, script_path, output_path, mode, cwd, terminal_cols, terminal_rows,
			status, session_name, pane_target, exit_code, reason, message,
			created_at, started_at, ended_at, updated_at
		FROM jobs WHERE job_id = ?",
            params![job_id],
            scan_job,
        )
        .optional()
        .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?
        .ok_or_else(ServerError::job_not_found)
    }

    pub fn list_jobs(&self, statuses: &[JobStatusName]) -> Result<Vec<JobRow>, ServerError> {
        let conn = self.conn.lock();
        let sql = if statuses.is_empty() {
            "SELECT job_id, script_path, output_path, mode, cwd,
			terminal_cols, terminal_rows, status, session_name, pane_target,
			exit_code, reason, message, created_at, started_at, ended_at, updated_at
			FROM jobs ORDER BY created_at DESC"
                .to_string()
        } else {
            let placeholders: Vec<String> = (1..=statuses.len()).map(|i| format!("?{i}")).collect();
            format!(
                "SELECT job_id, script_path, output_path, mode, cwd,
			terminal_cols, terminal_rows, status, session_name, pane_target,
			exit_code, reason, message, created_at, started_at, ended_at, updated_at
			FROM jobs WHERE status IN ({}) ORDER BY created_at DESC",
                placeholders.join(",")
            )
        };
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        let mapped = if statuses.is_empty() {
            stmt.query_map([], scan_job)
        } else {
            let vals: Vec<String> = statuses.iter().map(|s| s.as_str().to_string()).collect();
            stmt.query_map(params_from_iter(vals.iter()), scan_job)
        };
        let rows = mapped.map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?);
        }
        Ok(out)
    }

    pub fn delete_job(&self, job_id: &str) -> Result<(), ServerError> {
        let conn = self.conn.lock();
        let n = conn
            .execute("DELETE FROM jobs WHERE job_id = ?", params![job_id])
            .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        if n == 0 {
            return Err(ServerError::job_not_found());
        }
        Ok(())
    }

    pub fn transition_status(
        &self,
        job_id: &str,
        opts: TransitionOpts,
    ) -> Result<JobRow, ServerError> {
        let now = format_timestamp(std::time::SystemTime::now());
        let mut all: Vec<String> = opts
            .allowed_from
            .iter()
            .chain(opts.allow_override_from.iter())
            .map(|s| s.as_str().to_string())
            .collect();
        if all.is_empty() {
            all.push(opts.target.as_str().to_string());
        }
        let placeholders: String = (0..all.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let mut set_clauses = "status = ?, updated_at = ?, reason = ?, message = ?".to_string();
        let mut args: Vec<Box<dyn ToSql>> = vec![
            Box::new(opts.target.as_str().to_string()),
            Box::new(now.clone()),
            Box::new(opts.reason.clone()),
            Box::new(opts.message.clone()),
        ];
        if matches!(opts.target, JobStatusName::Starting | JobStatusName::Running) {
            set_clauses.push_str(", started_at = CASE WHEN started_at IS NULL THEN ? ELSE started_at END");
            args.push(Box::new(now.clone()));
        }
        if opts.target.is_terminal() {
            let ended = if opts.ended_at.is_empty() {
                now.clone()
            } else {
                opts.ended_at.clone()
            };
            set_clauses.push_str(
                ", ended_at = CASE WHEN ended_at IS NULL THEN ? ELSE ended_at END",
            );
            args.push(Box::new(ended));
            set_clauses.push_str(
                ", exit_code = CASE WHEN exit_code IS NULL THEN 0 ELSE exit_code END",
            );
        }
        let mut where_clause = format!("job_id = ? AND status IN ({placeholders})");
        args.push(Box::new(job_id.to_string()));
        for s in &all {
            args.push(Box::new(s.clone()));
        }
        if opts.require_exit_code_null {
            where_clause.push_str(" AND exit_code IS NULL");
        }
        let query = format!("UPDATE jobs SET {set_clauses} WHERE {where_clause}");
        {
            let conn = self.conn.lock();
            let mut stmt = conn
                .prepare(&query)
                .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
            let refs: Vec<&dyn ToSql> = args.iter().map(|a| a.as_ref()).collect();
            stmt.execute(refs.as_slice())
                .map_err(|e| ServerError::new(500, "internal_error", format!("transition status: {e}")))?;
        }
        self.get_job(job_id)
    }

    pub fn record_runner_exit(
        &self,
        job_id: &str,
        exit_code: i32,
        ended_at: &str,
    ) -> Result<(), ServerError> {
        let conn = self.conn.lock();
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM jobs WHERE job_id = ?",
                params![job_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        let Some(status) = status else {
            return Err(ServerError::job_not_found());
        };
        if JobStatusName::parse(&status).is_terminal() {
            return Ok(());
        }
        let n = conn
            .execute(
                "UPDATE jobs
		SET status = CASE WHEN status IN (?, ?, ?) THEN ? ELSE status END,
			exit_code = CASE WHEN status IN (?, ?, ?) THEN ? ELSE exit_code END,
			ended_at = CASE WHEN status IN (?, ?, ?) AND ended_at IS NULL THEN ? ELSE ended_at END,
			updated_at = CASE WHEN status IN (?, ?, ?) THEN ? ELSE updated_at END,
			reason = CASE WHEN status IN (?, ?, ?) THEN NULL ELSE reason END,
			message = CASE WHEN status IN (?, ?, ?) THEN NULL ELSE message END
		WHERE job_id = ? AND status IN (?, ?, ?)",
                params![
                    "created",
                    "starting",
                    "running",
                    "exited",
                    "created",
                    "starting",
                    "running",
                    exit_code,
                    "created",
                    "starting",
                    "running",
                    ended_at,
                    "created",
                    "starting",
                    "running",
                    ended_at,
                    "created",
                    "starting",
                    "running",
                    "created",
                    "starting",
                    "running",
                    job_id,
                    "created",
                    "starting",
                    "running",
                ],
            )
            .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
        if n == 0 {
            return Err(ServerError::job_not_found());
        }
        Ok(())
    }
}

fn scan_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    let mode: String = row.get(3)?;
    let status: String = row.get(7)?;
    Ok(JobRow {
        job_id: row.get(0)?,
        script_path: row.get(1)?,
        output_path: row.get(2)?,
        mode: crate::jobmode::parse(&mode).unwrap_or(Mode::Pty),
        cwd: row.get(4)?,
        terminal_cols: row.get(5)?,
        terminal_rows: row.get(6)?,
        status: JobStatusName::parse(&status),
        session_name: row.get(8)?,
        pane_target: row.get(9)?,
        exit_code: row.get(10)?,
        reason: row.get(11)?,
        message: row.get(12)?,
        created_at: row.get(13)?,
        started_at: row.get(14)?,
        ended_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}
