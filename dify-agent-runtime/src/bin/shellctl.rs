fn main() {
    let mut cfg = match dify_agent_runtime::server::config::default_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("shellctl: config: {e}");
            std::process::exit(1);
        }
    };
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 || args[1] != "serve" {
        eprintln!("Usage: shellctl serve [flags]");
        return;
    }
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--listen" if i + 1 < args.len() => {
                cfg.listen = args[i + 1].clone();
                i += 2;
            }
            "--state-dir" if i + 1 < args.len() => {
                cfg.state_dir = args[i + 1].clone().into();
                cfg.runtime_dir = cfg.state_dir.join("runtime");
                i += 2;
            }
            "--token" if i + 1 < args.len() => {
                cfg.auth_token = args[i + 1].clone();
                i += 2;
            }
            other => {
                eprintln!("shellctl: unknown flag {other}");
                std::process::exit(1);
            }
        }
    }
    let rt = tokio::runtime::Runtime::new().expect("tokio");
    rt.block_on(async move {
        let mut svc = dify_agent_runtime::server::Service::new(cfg.clone());
        if let Err(e) = svc.initialize() {
            eprintln!("shellctl: initialize: {e}");
            std::process::exit(1);
        }
        let svc = std::sync::Arc::new(svc);
        svc.start_background_gc();
        svc.start_background_pipe_monitor();
        let state = std::sync::Arc::new(dify_agent_runtime::server::api::AppState {
            service: svc,
            config: cfg,
            snapshot: dify_agent_runtime::server::snapshot_http::SnapshotGate::new(),
        });
        if let Err(e) = dify_agent_runtime::server::api::serve(state).await {
            eprintln!("shellctl: {e}");
            std::process::exit(1);
        }
    });
}
