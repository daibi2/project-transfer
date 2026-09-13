use dify_agent_runtime::cmdutil;
use dify_agent_runtime::runner_exit::record_runner_exit;

fn main() {
    let mut state_dir = String::new();
    let mut job_id = String::new();
    let mut exit_code = 0i32;
    let mut ended_at = String::new();
    let mut busy = 5000i32;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--state-dir" => state_dir = args.next().unwrap_or_default(),
            "--job-id" => job_id = args.next().unwrap_or_default(),
            "--exit-code" => exit_code = args.next().unwrap_or_default().parse().unwrap_or(0),
            "--ended-at" => ended_at = args.next().unwrap_or_default(),
            "--sqlite-busy-timeout-ms" => {
                busy = args.next().unwrap_or_default().parse().unwrap_or(5000)
            }
            _ => {}
        }
    }
    if state_dir.is_empty() || job_id.is_empty() || ended_at.is_empty() {
        cmdutil::handle_error(
            Some("missing flags"),
            1,
            "--state-dir, --job-id, and --ended-at are required",
        );
    }
    cmdutil::handle_result(
        record_runner_exit(&state_dir, &job_id, exit_code, &ended_at, busy),
        1,
        "record runner exit",
    );
}
