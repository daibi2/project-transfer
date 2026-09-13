#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = dify_agent_runtime::agentcli::cli::run_cli(&args).await {
        eprintln!("dify-agent: {e}");
        std::process::exit(1);
    }
}
