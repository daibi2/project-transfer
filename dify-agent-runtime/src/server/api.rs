use crate::server::config::{
    self, Config, DEFAULT_IDLE_FLUSH_SECONDS, DEFAULT_TIMEOUT_SECONDS,
};
use crate::server::service::Service;
use crate::server::snapshot_http::{
    self, json_error, json_ok, write_server_error, ApiBody, SnapshotGate,
};
use crate::server::types::*;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;

pub struct AppState {
    pub service: Arc<Service>,
    pub config: Config,
    pub snapshot: SnapshotGate,
}

pub async fn serve(state: Arc<AppState>) -> Result<(), String> {
    let addr: SocketAddr = state
        .config
        .listen
        .parse()
        .map_err(|e| format!("listen addr: {e}"))?;
    let listener = TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind: {e}"))?;
    eprintln!("shellctl serving on {}", state.config.listen);
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let io = TokioIo::new(stream);
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let svc = service_fn(move |req| {
                let state = Arc::clone(&state);
                async move { Ok::<_, std::convert::Infallible>(dispatch(state, req).await) }
            });
            if let Err(e) = http1::Builder::new()
                .keep_alive(true)
                .serve_connection(io, svc)
                .await
            {
                eprintln!("connection error: {e}");
            }
        });
    }
}

async fn dispatch(state: Arc<AppState>, req: Request<Incoming>) -> Response<ApiBody> {
    let start = Instant::now();
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let resp = route(state, req).await;
    eprintln!(
        "{method} {path} -> {} ({:?})",
        resp.status().as_u16(),
        start.elapsed()
    );
    resp
}

async fn route(state: Arc<AppState>, req: Request<Incoming>) -> Response<ApiBody> {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    if method == Method::GET && path == "/healthz" {
        return json_ok(&HealthResponse {
            status: config::HEALTH_STATUS.into(),
        });
    }

    let auth = &state.config.auth_token;
    if !auth.is_empty() {
        let expected = format!("Bearer {auth}");
        let got = req
            .headers()
            .get(hyper::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if got != expected {
            return json_error(401, "unauthorized", "Missing or invalid bearer token");
        }
    }

    if method == Method::POST && path == "/v1/jobs/run" {
        return handle_run(state, req).await;
    }
    if method == Method::GET && path == "/v1/jobs" {
        return handle_list(state, &query);
    }
    if method == Method::POST && path == "/v1/snapshot/save" {
        return snapshot_http::handle_save(req, &state.config, &state.snapshot).await;
    }
    if method == Method::POST && path == "/v1/snapshot/restore" {
        return snapshot_http::handle_restore(req, &state.config, &state.snapshot).await;
    }

    if let Some(rest) = path.strip_prefix("/v1/jobs/") {
        let parts: Vec<&str> = rest.split('/').collect();
        if parts.len() == 1 {
            let job_id = parts[0].to_string();
            if method == Method::GET {
                return handle_status(state, &job_id);
            }
            if method == Method::DELETE {
                return handle_delete(state, &job_id, &query);
            }
        }
        if parts.len() == 2 {
            let job_id = parts[0].to_string();
            match (method.clone(), parts[1]) {
                (Method::POST, "wait") => return handle_wait(state, &job_id, req).await,
                (Method::POST, "input") => return handle_input(state, &job_id, req).await,
                (Method::POST, "terminate") => return handle_terminate(state, &job_id, req).await,
                _ => {}
            }
        }
        if parts.len() == 3 && parts[1] == "log" && parts[2] == "tail" && method == Method::GET {
            return handle_tail(state, parts[0], &query);
        }
    }

    json_error(404, "not_found", "not found")
}

async fn read_json<T: serde::de::DeserializeOwned>(req: Request<Incoming>) -> Result<T, Response<ApiBody>> {
    let bytes = req
        .into_body()
        .collect()
        .await
        .map_err(|e| json_error(400, "invalid_request", e.to_string()))?
        .to_bytes();
    serde_json::from_slice(&bytes).map_err(|_| json_error(400, "invalid_request", "Invalid JSON body"))
}

async fn handle_run(state: Arc<AppState>, req: Request<Incoming>) -> Response<ApiBody> {
    let mut body: RunJobRequest = match read_json(req).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.script.is_empty() {
        return json_error(400, "invalid_request", "script is required");
    }
    match crate::jobmode::parse(body.mode.as_deref().unwrap_or("")) {
        Ok(m) => body.mode = Some(m.as_str().to_string()),
        Err(e) => return json_error(422, "validation_error", e),
    }
    if let Some(env) = &body.env {
        for (name, value) in env {
            if name.is_empty() {
                return json_error(422, "validation_error", "env names must be non-empty");
            }
            if name.contains('=') {
                return json_error(
                    422,
                    "validation_error",
                    format!("env name must not contain '=': {name:?}"),
                );
            }
            if name.contains('\0') || value.contains('\0') {
                return json_error(422, "validation_error", "env entries must not contain NUL");
            }
        }
    }
    let svc = Arc::clone(&state.service);
    match tokio::task::spawn_blocking(move || svc.run_job(&body)).await {
        Ok(Ok(result)) => json_ok(&result),
        Ok(Err(e)) => write_server_error(e),
        Err(e) => json_error(500, "internal_error", e.to_string()),
    }
}

async fn handle_wait(
    state: Arc<AppState>,
    job_id: &str,
    req: Request<Incoming>,
) -> Response<ApiBody> {
    let mut body: WaitJobRequest = match read_json(req).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.idle_flush_seconds == 0.0 {
        body.idle_flush_seconds = DEFAULT_IDLE_FLUSH_SECONDS;
    }
    let job_id = job_id.to_string();
    let svc = Arc::clone(&state.service);
    match tokio::task::spawn_blocking(move || svc.wait_job(&job_id, &body)).await {
        Ok(Ok(result)) => json_ok(&result),
        Ok(Err(e)) => write_server_error(e),
        Err(e) => json_error(500, "internal_error", e.to_string()),
    }
}

fn handle_tail(state: Arc<AppState>, job_id: &str, query: &str) -> Response<ApiBody> {
    let mut output_limit = state.config.default_output_limit_bytes;
    if let Some(v) = query_param(query, "output_limit") {
        if let Ok(n) = v.parse::<i64>() {
            if n > 0 {
                output_limit = n;
            }
        }
    }
    if output_limit > state.config.max_output_limit_bytes {
        output_limit = state.config.max_output_limit_bytes;
    }
    match state.service.tail_job(job_id, output_limit) {
        Ok(r) => json_ok(&r),
        Err(e) => write_server_error(e),
    }
}

fn handle_status(state: Arc<AppState>, job_id: &str) -> Response<ApiBody> {
    match state.service.get_job_status(job_id) {
        Ok(r) => json_ok(&r),
        Err(e) => write_server_error(e),
    }
}

fn handle_list(state: Arc<AppState>, query: &str) -> Response<ApiBody> {
    let status = query_param(query, "status");
    let mut limit = state.config.default_list_limit;
    if let Some(v) = query_param(query, "limit") {
        if let Ok(n) = v.parse::<i64>() {
            if n > 0 {
                limit = n;
            }
        }
    }
    if limit > state.config.max_list_limit {
        limit = state.config.max_list_limit;
    }
    match state.service.list_jobs(status.as_deref(), limit) {
        Ok(r) => json_ok(&r),
        Err(e) => write_server_error(e),
    }
}

async fn handle_input(
    state: Arc<AppState>,
    job_id: &str,
    req: Request<Incoming>,
) -> Response<ApiBody> {
    let mut body: InputJobRequest = match read_json(req).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    if body.idle_flush_seconds == 0.0 {
        body.idle_flush_seconds = DEFAULT_IDLE_FLUSH_SECONDS;
    }
    if body.timeout == 0.0 {
        body.timeout = DEFAULT_TIMEOUT_SECONDS;
    }
    let job_id = job_id.to_string();
    let svc = Arc::clone(&state.service);
    match tokio::task::spawn_blocking(move || svc.send_input(&job_id, &body)).await {
        Ok(Ok(r)) => json_ok(&r),
        Ok(Err(e)) => write_server_error(e),
        Err(e) => json_error(500, "internal_error", e.to_string()),
    }
}

async fn handle_terminate(
    state: Arc<AppState>,
    job_id: &str,
    req: Request<Incoming>,
) -> Response<ApiBody> {
    let body: TerminateJobRequest = read_json(req).await.unwrap_or_default();
    let mut grace = state.config.default_terminate_grace_seconds;
    if let Some(g) = body.grace_seconds {
        grace = g;
    }
    let job_id = job_id.to_string();
    let svc = Arc::clone(&state.service);
    match tokio::task::spawn_blocking(move || svc.terminate_job(&job_id, grace)).await {
        Ok(Ok(r)) => json_ok(&r),
        Ok(Err(e)) => write_server_error(e),
        Err(e) => json_error(500, "internal_error", e.to_string()),
    }
}

fn handle_delete(state: Arc<AppState>, job_id: &str, query: &str) -> Response<ApiBody> {
    let force = query_param(query, "force").as_deref() == Some("true");
    let mut grace = state.config.default_terminate_grace_seconds;
    if let Some(v) = query_param(query, "grace_seconds") {
        if let Ok(n) = v.parse::<f64>() {
            grace = n;
        }
    }
    match state.service.delete_job(job_id, force, grace) {
        Ok(r) => json_ok(&r),
        Err(e) => write_server_error(e),
    }
}

fn query_param(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == key {
            return Some(urlencoding_decode(v));
        }
    }
    None
}

fn urlencoding_decode(s: &str) -> String {
    url::form_urlencoded::parse(s.as_bytes())
        .next()
        .map(|(k, _)| k.into_owned())
        .unwrap_or_else(|| s.to_string())
}
