fn main() {
    let mut ready = String::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--ready-file" {
            ready = args.next().unwrap_or_default();
        }
    }
    let ready_opt = if ready.is_empty() { None } else { Some(ready.as_str()) };
    if let Err(e) = dify_agent_runtime::sanitize::run(ready_opt, std::io::stdin(), std::io::stdout()) {
        eprintln!("shellctl: sanitize-pty: {e}");
        std::process::exit(1);
    }
}
