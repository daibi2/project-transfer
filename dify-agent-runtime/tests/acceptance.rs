//! Container acceptance tests. Ignored unless SHELLCTL_GO_URL is set
//! (`make integration-test`). Env names kept for Makefile compatibility.

use serde_json::{json, Value};
use std::time::Duration;

fn base_url() -> String {
    std::env::var("SHELLCTL_GO_URL").unwrap_or_else(|_| "http://localhost:15005".into())
}
fn token() -> String {
    std::env::var("SHELLCTL_TEST_TOKEN").unwrap_or_else(|_| "test-token-123".into())
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap()
}

fn auth() -> String {
    format!("Bearer {}", token())
}

fn skip_if_unset() {
    if std::env::var("SHELLCTL_GO_URL").is_err() {
        panic!("ignored"); // #[ignore] handles skip; this is extra
    }
}

fn get(path: &str, with_auth: bool) -> reqwest::blocking::Response {
    let mut req = client().get(format!("{}{path}", base_url()));
    if with_auth {
        req = req.header("Authorization", auth());
    }
    req.send().unwrap()
}

fn post(path: &str, body: Value) -> reqwest::blocking::Response {
    client()
        .post(format!("{}{path}", base_url()))
        .header("Authorization", auth())
        .json(&body)
        .send()
        .unwrap()
}

fn run_job(body: Value) -> Value {
    let resp = post("/v1/jobs/run", body);
    assert_eq!(resp.status(), 200, "{}", resp.text().unwrap_or_default());
    resp.json().unwrap()
}

fn assert_done(v: &Value) {
    assert_eq!(v["done"], true, "{v}");
}

#[test]
#[ignore]
fn healthz() {
    skip_if_unset();
    let resp = get("/healthz", false);
    assert_eq!(resp.status(), 200);
    let v: Value = resp.json().unwrap();
    assert_eq!(v["status"], "ok");
}

#[test]
#[ignore]
fn auth_required() {
    let resp = get("/v1/jobs", false);
    assert_eq!(resp.status(), 401);
}

#[test]
#[ignore]
fn run_simple_script() {
    let result = run_job(json!({"script": "echo hello-world", "timeout": 10}));
    assert_done(&result);
    assert_eq!(result["exit_code"], 0);
    assert!(result["output"].as_str().unwrap().contains("hello-world"));
}

#[test]
#[ignore]
fn pty_merges_streams() {
    let result = run_job(json!({
        "script": "printf 'stdout-marker\\n'; printf 'stderr-marker\\n' >&2",
        "timeout": 10,
        "mode": "pty"
    }));
    assert_done(&result);
    let o = result["output"].as_str().unwrap();
    assert!(o.contains("stdout-marker") && o.contains("stderr-marker"));
}

#[test]
#[ignore]
fn stdio_separates_stdout() {
    let result = run_job(json!({
        "script": "printf '{\"ok\":true}'\nprintf 'warning' >&2",
        "mode": "stdio",
        "timeout": 10
    }));
    assert_done(&result);
    assert_eq!(result["output"], "{\"ok\":true}");
}

#[test]
#[ignore]
fn stdio_input_rejected() {
    let result = run_job(json!({"script": "sleep 60", "mode": "stdio", "timeout": 0.1}));
    let id = result["job_id"].as_str().unwrap();
    let resp = post(
        &format!("/v1/jobs/{id}/input"),
        json!({"text": "ignored\n", "offset": 0, "timeout": 1}),
    );
    assert_eq!(resp.status(), 409);
    let v: Value = resp.json().unwrap();
    assert_eq!(v["error"]["code"], "input_unsupported");
    let _ = post(&format!("/v1/jobs/{id}/terminate"), json!({"grace_seconds": 0}));
}

#[test]
#[ignore]
fn run_with_env_and_cwd() {
    let r = run_job(json!({
        "script": "echo $MY_VAR",
        "env": {"MY_VAR": "test-value-42"},
        "timeout": 10
    }));
    assert!(r["output"].as_str().unwrap().contains("test-value-42"));
    let r = run_job(json!({"script": "pwd", "cwd": "/tmp", "timeout": 10}));
    assert!(r["output"].as_str().unwrap().contains("/tmp"));
}

#[test]
#[ignore]
fn nonzero_exit() {
    let r = run_job(json!({"script": "exit 42", "timeout": 10}));
    assert_eq!(r["exit_code"], 42);
}

#[test]
#[ignore]
fn list_status_delete() {
    let r = run_job(json!({"script": "echo for-listing", "timeout": 10}));
    let id = r["job_id"].as_str().unwrap().to_string();
    let st = get(&format!("/v1/jobs/{id}"), true);
    assert_eq!(st.status(), 200);
    let list = get("/v1/jobs", true);
    let v: Value = list.json().unwrap();
    assert!(!v["jobs"].as_array().unwrap().is_empty());
    let del = client()
        .delete(format!("{}/v1/jobs/{id}", base_url()))
        .header("Authorization", auth())
        .send()
        .unwrap();
    assert_eq!(del.status(), 200);
}

#[test]
#[ignore]
fn invalid_cwd_and_not_found() {
    let resp = post("/v1/jobs/run", json!({"script": "true", "cwd": "/no/such", "timeout": 1}));
    assert_eq!(resp.status(), 400);
    let resp = get("/v1/jobs/does-not-exist", true);
    assert_eq!(resp.status(), 404);
}

#[test]
#[ignore]
fn landlock_home_and_usr() {
    let r = run_job(json!({"script": "echo ok > $HOME/ll.txt && cat $HOME/ll.txt", "timeout": 10}));
    assert_done(&r);
    assert!(r["output"].as_str().unwrap().contains("ok"));
    let r = run_job(json!({"script": "ls /usr/bin/bash", "timeout": 10}));
    assert_eq!(r["exit_code"], 0);
}
