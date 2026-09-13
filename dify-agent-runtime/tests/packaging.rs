use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn cargo_toml_declares_binaries_and_sqlite() {
    let data = fs::read_to_string(root().join("Cargo.toml")).unwrap();
    assert!(data.contains("name = \"dify-agent-runtime\""));
    assert!(data.contains("rusqlite"));
    for bin in [
        "shellctl",
        "shellctl-sanitize-pty",
        "shellctl-runner-exit",
        "shellctl-runner",
        "dify-agent",
    ] {
        assert!(
            data.contains(&format!("name = \"{bin}\"")),
            "missing bin {bin}"
        );
    }
}

#[test]
fn makefile_build_targets() {
    let data = fs::read_to_string(root().join("Makefile")).unwrap();
    for t in [
        "$(BIN_DIR)/shellctl",
        "$(BIN_DIR)/shellctl-sanitize-pty",
        "$(BIN_DIR)/shellctl-runner-exit",
        "$(BIN_DIR)/dify-agent",
        "$(BIN_DIR)/shellctl-runner",
    ] {
        assert!(data.contains(t), "missing {t}");
    }
    assert!(data.contains("$(CARGO) build") || data.contains("cargo build"));
}

#[test]
fn dockerfile_builds_and_copies_all_binaries() {
    let data = fs::read_to_string(root().join("docker/Dockerfile")).unwrap();
    for line in [
        "cargo build --release",
        "COPY --from=rust-builder /src/target/release/shellctl /usr/local/bin/shellctl",
        "COPY --from=rust-builder /src/target/release/shellctl-sanitize-pty /usr/local/bin/shellctl-sanitize-pty",
        "COPY --from=rust-builder /src/target/release/shellctl-runner-exit /usr/local/bin/shellctl-runner-exit",
        "COPY --from=rust-builder /src/target/release/dify-agent /usr/local/bin/dify-agent",
        "COPY --from=rust-builder /src/target/release/shellctl-runner /usr/local/bin/shellctl-runner",
    ] {
        assert!(data.contains(line), "missing {line}");
    }
    assert!(data.contains(r#"CMD ["shellctl", "serve", "--listen", "0.0.0.0:5004"]"#));
    assert!(data.contains("EXPOSE 5004"));
    assert!(data.contains("USER dify"));
    assert!(data.contains("bash"));
    assert!(data.contains("git"));
    assert!(data.contains("jq"));
    assert!(data.contains("tmux"));
    assert!(!data.contains("shellctl-server"));
}
