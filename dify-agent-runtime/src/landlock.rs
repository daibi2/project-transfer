use crate::envvar;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    pub home: String,
    pub cwd: String,
    pub job_dir: String,
    pub rw_paths: Vec<String>,
    pub ro_paths: Vec<String>,
    pub rw_dev_paths: Vec<String>,
}

pub fn default_ro_paths() -> Vec<String> {
    [
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc",
        "/proc",
        "/opt/dify-agent-tools",
        "/opt/homebrew",
        "/snap",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn default_rw_dev_paths() -> Vec<String> {
    [
        "/dev/null",
        "/dev/zero",
        "/dev/urandom",
        "/dev/random",
        "/dev/tty",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect()
}

pub fn default_config(home: &str, cwd: &str, job_dir: &str) -> Config {
    Config {
        home: home.to_string(),
        cwd: cwd.to_string(),
        job_dir: job_dir.to_string(),
        rw_paths: Vec::new(),
        ro_paths: default_ro_paths(),
        rw_dev_paths: default_rw_dev_paths(),
    }
}

pub fn config_from_env(home: &str, cwd: &str, job_dir: &str) -> Config {
    let mut cfg = default_config(home, cwd, job_dir);
    if let Ok(v) = std::env::var(envvar::ENV_RW_PATHS) {
        cfg.rw_paths = split_paths(&v);
    }
    if let Ok(v) = std::env::var(envvar::ENV_RO_PATHS) {
        cfg.ro_paths = split_paths(&v);
    }
    if let Ok(v) = std::env::var(envvar::ENV_RW_DEV_PATHS) {
        cfg.rw_dev_paths = split_paths(&v);
    }
    cfg
}

fn split_paths(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

pub fn filter_existing(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|p| Path::new(p.as_str()).exists())
        .cloned()
        .collect()
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use landlock::{
        Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, ABI,
        RulesetError,
    };

    pub fn restrict(cfg: &Config) -> Result<(), String> {
        apply(cfg).map_err(|e| e.to_string())
    }

    fn apply(cfg: &Config) -> Result<(), RulesetError> {
        let abi = ABI::V1;
        let mut ro_paths = filter_existing(&cfg.ro_paths);
        if !cfg.job_dir.is_empty() {
            ro_paths.push(cfg.job_dir.clone());
        }
        let mut rw_dirs = vec![cfg.home.clone()];
        if !cfg.cwd.is_empty() && cfg.cwd != cfg.home {
            rw_dirs.push(cfg.cwd.clone());
        }
        rw_dirs.extend(filter_existing(&cfg.rw_paths));
        let rw_dev = filter_existing(&cfg.rw_dev_paths);

        let mut ruleset = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))?
            .create()?;

        let rw_access = AccessFs::from_all(abi);
        for p in &rw_dirs {
            if let Ok(fd) = PathFd::new(p) {
                ruleset = ruleset.add_rule(PathBeneath::new(fd, rw_access))?;
            }
        }
        // Read + execute, matching go-landlock RODirs.
        let ro_access = AccessFs::from_read(abi) | AccessFs::Execute;
        for p in &ro_paths {
            if let Ok(fd) = PathFd::new(p) {
                ruleset = ruleset.add_rule(PathBeneath::new(fd, ro_access))?;
            }
        }
        for p in &rw_dev {
            if let Ok(fd) = PathFd::new(p) {
                ruleset = ruleset.add_rule(PathBeneath::new(fd, rw_access))?;
            }
        }
        ruleset.restrict_self()?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub use linux::restrict;

#[cfg(not(target_os = "linux"))]
pub fn restrict(_cfg: &Config) -> Result<(), String> {
    Err("landlock: not supported by kernel".into())
}
