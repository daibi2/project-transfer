use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

const NONTERMINAL: [&str; 3] = ["created", "starting", "running"];
const TERMINAL: [&str; 4] = ["exited", "terminated", "failed", "lost"];

pub fn record_runner_exit(
    state_dir: &str,
    job_id: &str,
    exit_code: i32,
    ended_at: &str,
    busy_timeout_ms: i32,
) -> Result<(), String> {
    let db_path = Path::new(state_dir).join("shellctl.db");
    let conn = Connection::open(&db_path).map_err(|e| format!("open database: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms as u64))
        .map_err(|e| format!("busy timeout: {e}"))?;
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.busy_timeout(std::time::Duration::from_millis(busy_timeout_ms as u64))
        .ok();

    let status: Option<String> = conn
        .query_row(
            "SELECT status FROM jobs WHERE job_id = ?",
            params![job_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("query job status: {e}"))?;

    let Some(status) = status else {
        return Err(format!("unknown job id: {job_id}"));
    };
    if is_terminal(&status) {
        return Ok(());
    }

    let n = conn
        .execute(
            "
		UPDATE jobs
		SET status = CASE
				WHEN status IN (?, ?, ?) THEN ?
				ELSE status
			END,
			exit_code = CASE
				WHEN status IN (?, ?, ?) THEN ?
				ELSE exit_code
			END,
			ended_at = CASE
				WHEN status IN (?, ?, ?) THEN ?
				ELSE ended_at
			END,
			updated_at = CASE
				WHEN status IN (?, ?, ?) THEN ?
				ELSE updated_at
			END,
			reason = CASE
				WHEN status IN (?, ?, ?) THEN NULL
				ELSE reason
			END,
			message = CASE
				WHEN status IN (?, ?, ?) THEN NULL
				ELSE message
			END
		WHERE job_id = ?",
            params![
                NONTERMINAL[0],
                NONTERMINAL[1],
                NONTERMINAL[2],
                "exited",
                NONTERMINAL[0],
                NONTERMINAL[1],
                NONTERMINAL[2],
                exit_code,
                NONTERMINAL[0],
                NONTERMINAL[1],
                NONTERMINAL[2],
                ended_at,
                NONTERMINAL[0],
                NONTERMINAL[1],
                NONTERMINAL[2],
                ended_at,
                NONTERMINAL[0],
                NONTERMINAL[1],
                NONTERMINAL[2],
                NONTERMINAL[0],
                NONTERMINAL[1],
                NONTERMINAL[2],
                job_id,
            ],
        )
        .map_err(|e| format!("update job: {e}"))?;
    if n == 0 {
        return Err(format!("unknown job id: {job_id}"));
    }
    Ok(())
}

pub fn is_terminal(status: &str) -> bool {
    TERMINAL.contains(&status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_db(dir: &std::path::Path) {
        let db_path = dir.join("shellctl.db");
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            "
		CREATE TABLE jobs (
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
		);
		INSERT INTO jobs (job_id, script_path, output_path, cwd, status, session_name, pane_target, created_at, updated_at)
		VALUES ('test-job', 's', 'o', '/tmp', 'running', 'sess', 'pane', '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z');
		",
        )
        .unwrap();
    }

    #[test]
    fn record_running() {
        let dir = tempdir().unwrap();
        setup_db(dir.path());
        record_runner_exit(dir.path().to_str().unwrap(), "test-job", 0, "2025-01-15T12:00:00Z", 5000)
            .unwrap();
        let conn = Connection::open(dir.path().join("shellctl.db")).unwrap();
        let (status, code): (String, i32) = conn
            .query_row("SELECT status, exit_code FROM jobs WHERE job_id = ?", ["test-job"], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(status, "exited");
        assert_eq!(code, 0);
    }

    #[test]
    fn record_nonzero() {
        let dir = tempdir().unwrap();
        setup_db(dir.path());
        record_runner_exit(dir.path().to_str().unwrap(), "test-job", 42, "2025-01-15T12:00:00Z", 5000)
            .unwrap();
        let conn = Connection::open(dir.path().join("shellctl.db")).unwrap();
        let code: i32 = conn
            .query_row("SELECT exit_code FROM jobs WHERE job_id = ?", ["test-job"], |r| r.get(0))
            .unwrap();
        assert_eq!(code, 42);
    }

    #[test]
    fn job_not_found() {
        let dir = tempdir().unwrap();
        setup_db(dir.path());
        assert!(record_runner_exit(
            dir.path().to_str().unwrap(),
            "nonexistent-job",
            0,
            "2025-01-15T12:00:00Z",
            5000
        )
        .is_err());
    }

    #[test]
    fn db_not_found() {
        let dir = tempdir().unwrap();
        assert!(record_runner_exit(
            dir.path().to_str().unwrap(),
            "test-job",
            0,
            "2025-01-15T12:00:00Z",
            5000
        )
        .is_err());
    }

    #[test]
    fn terminal_idempotent() {
        let dir = tempdir().unwrap();
        setup_db(dir.path());
        let conn = Connection::open(dir.path().join("shellctl.db")).unwrap();
        conn.execute(
            "UPDATE jobs SET status='terminated', exit_code=137, ended_at='2025-01-01T00:01:00Z' WHERE job_id='test-job'",
            [],
        )
        .unwrap();
        drop(conn);
        record_runner_exit(dir.path().to_str().unwrap(), "test-job", 0, "2025-01-15T12:00:00Z", 5000)
            .unwrap();
        let conn = Connection::open(dir.path().join("shellctl.db")).unwrap();
        let (status, code): (String, i32) = conn
            .query_row("SELECT status, exit_code FROM jobs WHERE job_id = ?", ["test-job"], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(status, "terminated");
        assert_eq!(code, 137);
    }

    #[test]
    fn terminal_classification() {
        for s in ["exited", "terminated", "failed", "lost"] {
            assert!(is_terminal(s), "{s}");
        }
        for s in ["created", "starting", "running"] {
            assert!(!is_terminal(s), "{s}");
        }
    }
}
