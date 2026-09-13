use std::env;

pub const ENV_ENABLE_PATH_ISOLATION: &str = "SHELLCTL_ENABLE_PATH_ISOLATION";
pub const ENV_RW_PATHS: &str = "SHELLCTL_LANDLOCK_RW_PATHS";
pub const ENV_RO_PATHS: &str = "SHELLCTL_LANDLOCK_RO_PATHS";
pub const ENV_RW_DEV_PATHS: &str = "SHELLCTL_LANDLOCK_RW_DEV_PATHS";
pub const ENV_AGENT_STUB_API_BASE_URL: &str = "DIFY_AGENT_STUB_API_BASE_URL";
pub const ENV_AGENT_STUB_AUTH_JWE: &str = "DIFY_AGENT_STUB_AUTH_JWE";

pub fn path_isolation_enabled() -> bool {
    match env::var(ENV_ENABLE_PATH_ISOLATION) {
        Err(_) => true,
        Ok(v) => v == "true",
    }
}

pub fn set_path_isolation(enabled: bool) {
    env::set_var(
        ENV_ENABLE_PATH_ISOLATION,
        if enabled { "true" } else { "false" },
    );
}
