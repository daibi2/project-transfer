fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = dify_agent_runtime::runner::run_from_args(&args);
    std::process::exit(code);
}
