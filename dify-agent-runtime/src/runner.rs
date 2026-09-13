//! shellctl-runner parent/child process logic.

use crate::envvar;
use crate::jobmode::{self, Mode};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const START_GATE_POLL: Duration = Duration::from_millis(5);

pub fn run_from_args(args: &[String]) -> i32 {
    if args.len() > 1 && args[1] == "--exec" {
        child_mode(args);
    }
    parent_mode(args)
}

fn parent_mode(args: &[String]) -> i32 {
    if args.len() < 4 {
        eprintln!("shellctl: usage: shellctl-runner <job_dir> <job_id> <cwd> [pty|stdio]: bad args");
        return 125;
    }
    let job_dir = PathBuf::from(&args[1]);
    let cwd = args[3].clone();
    let mode_raw = if args.len() >= 5 { args[4].as_str() } else { "" };
    let mode = match jobmode::parse(mode_raw) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("shellctl: parse job mode: {e}");
            return 125;
        }
    };

    let script_path = job_dir.join("script");
    let env_path = job_dir.join(".job-env.json");
    let start_gate = job_dir.join("start-gate");
    loop {
        if start_gate.exists() {
            break;
        }
        thread::sleep(START_GATE_POLL);
    }

    let mut env = filter_env(std::env::vars(), &[
        "TMUX",
        "SHELLCTL_STATE_DIR",
        "SHELLCTL_RUNTIME_DIR",
        "SHELLCTL_TMUX_SOCKET",
        "SHELLCTL_RUNNER",
        "SHELLCTL_AUTH_TOKEN",
    ]);
    if let Some(overlay) = load_env_json(&env_path) {
        merge_env(&mut env, overlay);
    }
    merge_env(
        &mut env,
        [
            ("TMPDIR".into(), cwd.clone()),
            ("TMP".into(), cwd.clone()),
            ("TEMP".into(), cwd.clone()),
        ]
        .into_iter()
        .collect(),
    );

    if let Some(home) = env_get(&env, "HOME") {
        if let Err(e) = fs::create_dir_all(&home) {
            eprintln!("shellctl: mkdir HOME {home}: {e}");
            return 125;
        }
    }

    let enable_isolation = envvar::path_isolation_enabled();
    let self_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("shellctl-runner"));
    let mut child_args = vec!["--exec".to_string()];
    if enable_isolation {
        child_args.push("--landlock".into());
    }
    child_args.push(script_path.to_string_lossy().into_owned());
    child_args.push(cwd.clone());

    let mut cmd = Command::new(self_exe);
    cmd.args(&child_args).current_dir(&cwd);
    cmd.env_clear();
    for (k, v) in &env {
        cmd.env(k, v);
    }

    run_command_and_record_exit(&mut cmd, &job_dir, mode)
}

pub fn run_command_and_record_exit(cmd: &mut Command, job_dir: &Path, mode: Mode) -> i32 {
    let exit_code = if mode == Mode::Stdio {
        run_stdio(cmd, job_dir)
    } else {
        cmd.stdin(Stdio::inherit());
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());
        command_exit_code(cmd.status())
    };
    let ended_at = chrono_utc_now();
    write_atomic(&job_dir.join("runner-exit-code"), &format!("{exit_code}"));
    write_atomic(&job_dir.join("runner-ended-at"), &ended_at);
    exit_code
}

fn run_stdio(cmd: &mut Command, job_dir: &Path) -> i32 {
    let output_file = match open_capture(&job_dir.join("output.log")) {
        Ok(f) => f,
        Err(e) => return runner_error("open stdout capture", &e),
    };
    let stderr_file = match open_capture(&job_dir.join("stderr.log")) {
        Ok(f) => f,
        Err(e) => return runner_error("open stderr capture", &e),
    };

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return runner_error("start child", &e),
    };
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let e1 = errors.clone();
    let t1 = thread::spawn(move || {
        let mut f = output_file;
        if let Err(e) = io::copy(&mut { stdout }, &mut f) {
            e1.lock().unwrap().push(format!("copy stdout: {e}"));
        }
    });
    let e2 = errors.clone();
    let t2 = thread::spawn(move || {
        let mut f = stderr_file;
        if let Err(e) = io::copy(&mut { stderr }, &mut f) {
            e2.lock().unwrap().push(format!("copy stderr: {e}"));
        }
    });

    let wait_code = command_exit_code(child.wait());
    let _ = t1.join();
    let _ = t2.join();
    let errs = errors.lock().unwrap();
    for e in errs.iter() {
        eprintln!("shellctl-runner: {e}");
    }
    if !errs.is_empty() {
        return 125;
    }
    wait_code
}

fn open_capture(path: &Path) -> io::Result<File> {
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(f)
}

fn runner_error(op: &str, err: &dyn std::fmt::Display) -> i32 {
    eprintln!("shellctl-runner: {op}: {err}");
    125
}

fn command_exit_code(status: io::Result<std::process::ExitStatus>) -> i32 {
    match status {
        Ok(s) => s.code().unwrap_or(125),
        Err(_) => 125,
    }
}

fn child_mode(args: &[String]) -> ! {
    let mut rest = &args[2..];
    let mut apply_landlock = false;
    if !rest.is_empty() && rest[0] == "--landlock" {
        apply_landlock = true;
        rest = &rest[1..];
    }
    if rest.len() < 2 {
        eprintln!("shellctl: --exec: missing script_path and cwd: bad args");
        std::process::exit(125);
    }
    let script_path = PathBuf::from(&rest[0]);
    let cwd = &rest[1];
    if let Err(e) = std::env::set_current_dir(cwd) {
        eprintln!("shellctl: chdir {cwd}: {e}");
        std::process::exit(111);
    }
    let argv = build_exec_argv(&script_path);
    let binary = match which(&argv[0]) {
        Some(b) => b,
        None => {
            eprintln!("shellctl: {}: not found: ", argv[0]);
            std::process::exit(127);
        }
    };
    if apply_landlock {
        let home = std::env::var("HOME").unwrap_or_default();
        let job_dir = script_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cfg = crate::landlock::config_from_env(&home, cwd, &job_dir);
        if let Err(e) = crate::landlock::restrict(&cfg) {
            eprintln!(
                "shellctl-runner: WARNING: {e} — running without filesystem isolation"
            );
        }
    }
    let err = Command::new(&binary).args(&argv[1..]).exec();
    eprintln!("shellctl: exec {}: {err}", binary.display());
    std::process::exit(126);
}

fn build_exec_argv(script_path: &Path) -> Vec<String> {
    let path = script_path.to_string_lossy().into_owned();
    if let Ok(mut f) = File::open(script_path) {
        let mut buf = [0u8; 2];
        if f.read(&mut buf).unwrap_or(0) == 2 && &buf == b"#!" {
            let _ = fs::set_permissions(
                script_path,
                {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::Permissions::from_mode(0o755)
                    }
                    #[cfg(not(unix))]
                    {
                        fs::metadata(script_path).unwrap().permissions()
                    }
                },
            );
            return vec![path];
        }
    }
    vec!["sh".into(), path]
}

fn which(cmd: &str) -> Option<PathBuf> {
    let p = Path::new(cmd);
    if p.components().count() > 1 || cmd.contains('/') {
        if p.exists() {
            return Some(p.to_path_buf());
        }
        return None;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    for dir in path.split(':') {
        let cand = Path::new(dir).join(cmd);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn load_env_json(path: &Path) -> Option<std::collections::HashMap<String, String>> {
    let data = match fs::read_to_string(path) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("shellctl: read env json {}: {e}", path.display());
            std::process::exit(125);
        }
    };
    match serde_json::from_str::<Value>(&data) {
        Ok(Value::Object(map)) => {
            let mut out = std::collections::HashMap::new();
            for (k, v) in map {
                if let Some(s) = v.as_str() {
                    out.insert(k, s.to_string());
                }
            }
            Some(out)
        }
        Ok(_) | Err(_) => {
            eprintln!("shellctl: parse env json {}: invalid", path.display());
            std::process::exit(125);
        }
    }
}

fn filter_env(
    vars: impl Iterator<Item = (String, String)>,
    remove: &[&str],
) -> Vec<(String, String)> {
    vars.filter(|(k, _)| !remove.iter().any(|r| k == r))
        .collect()
}

fn merge_env(env: &mut Vec<(String, String)>, overlay: std::collections::HashMap<String, String>) {
    for (k, v) in overlay {
        if let Some(slot) = env.iter_mut().find(|(ek, _)| *ek == k) {
            slot.1 = v;
        } else {
            env.push((k, v));
        }
    }
}

fn env_get(env: &[(String, String)], key: &str) -> Option<String> {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

fn write_atomic(dest: &Path, value: &str) {
    let tmp = dest.with_extension(format!("tmp.{}", std::process::id()));
    // Go uses dest.tmp.<pid>
    let tmp = PathBuf::from(format!("{}.tmp.{}", dest.display(), std::process::id()));
    if let Err(e) = fs::write(&tmp, format!("{value}\n")) {
        eprintln!("shellctl-runner: write {}: {e}", tmp.display());
        return;
    }
    if let Err(e) = fs::rename(&tmp, dest) {
        eprintln!(
            "shellctl-runner: rename {} -> {}: {e}",
            tmp.display(),
            dest.display()
        );
    }
}

fn chrono_utc_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_unix_utc(secs)
}

pub fn format_unix_for_ts(secs: u64) -> String {
    format_unix_utc(secs)
}

fn format_unix_utc(secs: u64) -> String {
    // YYYY-MM-DDTHH:MM:SSZ
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}
