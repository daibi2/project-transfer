use super::config;
use super::connect::run_connect;
use super::env::{has_environment, read_environment};
use super::file;
use clap::{Arg, ArgAction, Command};
use std::io::Write;

const HELP_JSON: &str = include_str!("../../../dify-agent/src/dify_agent/layers/_agent_cli_help.json");

fn root() -> Command {
    Command::new("dify-agent")
        .about("Forward shell-visible dify-agent commands to the Dify Agent Stub server.")
        .subcommand_required(false)
        .arg_required_else_help(false)
        .disable_help_subcommand(true)
        .subcommand(connect_cmd())
        .subcommand(file_cmd())
        .subcommand(config_cmd())
}

fn connect_cmd() -> Command {
    Command::new("connect")
        .about("Establish one Agent Stub connection using the current environment.")
        .allow_hyphen_values(true)
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help("Emit the connection response as JSON."),
        )
        .arg(Arg::new("argv").num_args(0..).trailing_var_arg(true))
}

fn file_cmd() -> Command {
    Command::new("file")
        .about("Upload or download workflow files through the Agent Stub.")
        .subcommand(
            Command::new("upload")
                .about("Upload one sandbox-local file as a ToolFile output reference.")
                .arg(Arg::new("PATH").required(true))
                .arg(
                    Arg::new("no-download-link")
                        .long("no-download-link")
                        .action(ArgAction::SetTrue)
                        .help("Skip creating a public download link after upload."),
                ),
        )
        .subcommand(
            Command::new("download")
                .about("Download one workflow file mapping into the local sandbox directory.")
                .arg(Arg::new("TRANSFER_METHOD").required(true))
                .arg(Arg::new("REFERENCE_OR_URL").required(true))
                .arg(
                    Arg::new("to")
                        .long("to")
                        .help("Local directory for the downloaded file.")
                        .num_args(1),
                ),
        )
        .subcommand(
            Command::new("public-url")
                .about("Create a browser-visible download URL for an existing ToolFile reference.")
                .arg(Arg::new("REFERENCE").required(true)),
        )
}

fn config_cmd() -> Command {
    let skills_pull = Command::new("pull")
        .about("Pull one or all visible config skills into ./.dify_conf/skills by default.")
        .arg(Arg::new("NAME").num_args(0..))
        .arg(Arg::new("to").long("to").help("Local directory for pulled config skills.").num_args(1))
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue).help("Emit the pull result as JSON."));
    let files_pull = Command::new("pull")
        .about("Pull one or all visible config files into ./.dify_conf/files by default.")
        .arg(Arg::new("NAME").num_args(0..))
        .arg(Arg::new("to").long("to").help("Local directory for pulled config files.").num_args(1))
        .arg(Arg::new("json").long("json").action(ArgAction::SetTrue).help("Emit the pull result as JSON."));

    Command::new("config")
        .about("Inspect or update Agent Soul-backed config assets through the Agent Stub.")
        .subcommand(Command::new("manifest").about("Show the current visible Agent config manifest as JSON."))
        .subcommand(
            Command::new("skills")
                .about("Pull or update config skills through the Agent Stub.")
                .subcommand(skills_pull.clone())
                .subcommand(
                    Command::new("push")
                        .about("Upload one or more local skill directories into the current config manifest.")
                        .arg(Arg::new("PATH").required(true).num_args(1..)),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete one or more config skills by name without touching local directories.")
                        .arg(Arg::new("NAME").required(true).num_args(1..)),
                ),
        )
        .subcommand(
            Command::new("skill")
                .hide(true)
                .about("Pull config skills through the Agent Stub.")
                .subcommand(skills_pull),
        )
        .subcommand(
            Command::new("files")
                .about("Pull or update config files through the Agent Stub.")
                .subcommand(files_pull.clone())
                .subcommand(
                    Command::new("push")
                        .about("Upload one or more local files into the current config manifest.")
                        .arg(Arg::new("PATH").required(true).num_args(1..)),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete one or more config files by name without touching local files.")
                        .arg(Arg::new("NAME").required(true).num_args(1..)),
                ),
        )
        .subcommand(
            Command::new("file")
                .hide(true)
                .about("Pull config files through the Agent Stub.")
                .subcommand(files_pull),
        )
        .subcommand(
            Command::new("env")
                .about("Update config env variables visible to the current run.")
                .subcommand(
                    Command::new("push")
                        .about("Set or delete config env entries from one local dotenv file or stdin.")
                        .arg(Arg::new("PATH").required(true)),
                ),
        )
        .subcommand(
            Command::new("note")
                .about("Pull or update the current config note.")
                .subcommand(
                    Command::new("pull")
                        .about("Export the current config note into ./.dify_conf/note.md by default.")
                        .arg(Arg::new("to").long("to").help("Local markdown file path.").num_args(1)),
                )
                .subcommand(
                    Command::new("push")
                        .about("Replace the current config note from one local text file or stdin.")
                        .arg(Arg::new("PATH").num_args(0..=1)),
                ),
        )
}

fn known_root(name: &str) -> bool {
    matches!(name, "config" | "connect" | "file")
}

pub async fn run_cli(args: &[String]) -> Result<(), String> {
    if args.len() == 1 && args[0] == "__dump-cli-help" {
        print!("{HELP_JSON}");
        return Ok(());
    }
    if !args.is_empty() && args[0] == "connect" && !is_help_request(&args[1..]) {
        let (json_output, forwarded) = parse_connect_args(&args[1..]);
        return run_connect_args(forwarded, json_output).await;
    }
    let (json_output, forwarded) = extract_root_json_flag(args);
    if is_unknown_bare(&forwarded) {
        if !has_environment() {
            let mut cmd = root();
            let _ = cmd.print_help();
        }
        return run_connect_args(forwarded, json_output).await;
    }
    dispatch(args).await
}

async fn run_connect_args(forwarded: Vec<String>, json_output: bool) -> Result<(), String> {
    let env = read_environment()?;
    run_connect(&env, &forwarded, json_output).await
}

fn parse_connect_args(argv: &[String]) -> (bool, Vec<String>) {
    let mut remaining = argv.to_vec();
    let mut json_output = false;
    if remaining.first().map(|s| s.as_str()) == Some("--json") {
        json_output = true;
        remaining.remove(0);
    }
    if remaining.first().map(|s| s.as_str()) == Some("--") {
        remaining.remove(0);
    }
    (json_output, remaining)
}

fn extract_root_json_flag(argv: &[String]) -> (bool, Vec<String>) {
    if argv.len() >= 2 && argv[0] == "--json" && !known_root(&argv[1]) {
        return (true, argv[1..].to_vec());
    }
    (false, argv.to_vec())
}

fn is_unknown_bare(argv: &[String]) -> bool {
    argv.first()
        .map(|f| !known_root(f) && !f.starts_with('-'))
        .unwrap_or(false)
}

fn is_help_request(argv: &[String]) -> bool {
    argv.iter().any(|v| v == "--help" || v == "-h")
}

async fn dispatch(args: &[String]) -> Result<(), String> {
    // Let clap handle --help; for execution parse matches.
    if args.iter().any(|a| a == "--help" || a == "-h") || args.is_empty() {
        let mut cmd = root();
        let mut argv = vec!["dify-agent".into()];
        argv.extend(args.iter().cloned());
        match cmd.try_get_matches_from_mut(argv) {
            Ok(_) => {}
            Err(e) => {
                let _ = e.print();
                return Ok(());
            }
        }
        return Ok(());
    }
    let cmd = root();
    let mut argv = vec!["dify-agent".into()];
    argv.extend(args.iter().cloned());
    let matches = cmd
        .try_get_matches_from(argv)
        .map_err(|e| e.to_string())?;
    match matches.subcommand() {
        Some(("connect", m)) => {
            let json_output = m.get_flag("json");
            let forwarded: Vec<String> = m
                .get_many::<String>("argv")
                .map(|v| v.cloned().collect())
                .unwrap_or_default();
            run_connect_args(forwarded, json_output).await
        }
        Some(("file", m)) => dispatch_file(m).await,
        Some(("config", m)) => dispatch_config(m).await,
        _ => {
            let mut cmd = root();
            let _ = cmd.print_help();
            Ok(())
        }
    }
}

async fn dispatch_file(m: &clap::ArgMatches) -> Result<(), String> {
    let env = read_environment()?;
    match m.subcommand() {
        Some(("upload", sm)) => {
            let path = sm.get_one::<String>("PATH").unwrap();
            let no = sm.get_flag("no-download-link");
            file::run_file_upload(&env, path, no).await
        }
        Some(("download", sm)) => {
            let tm = sm.get_one::<String>("TRANSFER_METHOD").unwrap();
            let r = sm.get_one::<String>("REFERENCE_OR_URL").unwrap();
            let to = sm.get_one::<String>("to").map(|s| s.as_str()).unwrap_or("");
            file::run_file_download(&env, tm, r, to).await
        }
        Some(("public-url", sm)) => {
            let r = sm.get_one::<String>("REFERENCE").unwrap();
            file::run_file_public_url(&env, r).await
        }
        _ => Err("file subcommand required".into()),
    }
}

async fn dispatch_config(m: &clap::ArgMatches) -> Result<(), String> {
    let env = read_environment()?;
    match m.subcommand() {
        Some(("manifest", _)) => config::run_config_manifest(&env).await,
        Some(("skills", sm)) | Some(("skill", sm)) => match sm.subcommand() {
            Some(("pull", pm)) => {
                let names = pm
                    .get_many::<String>("NAME")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                let to = pm.get_one::<String>("to").cloned().unwrap_or_default();
                let json_output = pm.get_flag("json");
                config::run_config_skills_pull(&env, names, to, json_output).await
            }
            Some(("push", pm)) => {
                let paths = pm
                    .get_many::<String>("PATH")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                config::run_config_skills_push(&env, paths).await
            }
            Some(("delete", pm)) => {
                let names = pm
                    .get_many::<String>("NAME")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                config::run_config_skills_delete(&env, names).await
            }
            _ => Err("skills subcommand required".into()),
        },
        Some(("files", sm)) | Some(("file", sm)) => match sm.subcommand() {
            Some(("pull", pm)) => {
                let names = pm
                    .get_many::<String>("NAME")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                let to = pm.get_one::<String>("to").cloned().unwrap_or_default();
                let json_output = pm.get_flag("json");
                config::run_config_files_pull(&env, names, to, json_output).await
            }
            Some(("push", pm)) => {
                let paths = pm
                    .get_many::<String>("PATH")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                config::run_config_files_push(&env, paths).await
            }
            Some(("delete", pm)) => {
                let names = pm
                    .get_many::<String>("NAME")
                    .map(|v| v.cloned().collect())
                    .unwrap_or_default();
                config::run_config_files_delete(&env, names).await
            }
            _ => Err("files subcommand required".into()),
        },
        Some(("env", sm)) => match sm.subcommand() {
            Some(("push", pm)) => {
                let p = pm.get_one::<String>("PATH").unwrap().clone();
                config::run_config_env_push(&env, p).await
            }
            _ => Err("env subcommand required".into()),
        },
        Some(("note", sm)) => match sm.subcommand() {
            Some(("pull", pm)) => {
                let to = pm.get_one::<String>("to").cloned().unwrap_or_default();
                config::run_config_note_pull(&env, to).await
            }
            Some(("push", pm)) => {
                let p = pm.get_one::<String>("PATH").cloned().unwrap_or_default();
                config::run_config_note_push(&env, p).await
            }
            _ => Err("note subcommand required".into()),
        },
        _ => Err("config subcommand required".into()),
    }
}

pub fn dump_cli_help(w: &mut dyn Write) -> std::io::Result<()> {
    w.write_all(HELP_JSON.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_matches_checked_in() {
        let mut buf = Vec::new();
        dump_cli_help(&mut buf).unwrap();
        assert_eq!(buf, HELP_JSON.as_bytes());
        let table: serde_json::Value = serde_json::from_slice(&buf).unwrap();
        for key in [
            "config",
            "config manifest",
            "config skills pull",
            "config files pull",
            "config note pull",
            "config note push",
            "config env push",
            "config files push",
            "config files delete",
            "config skills push",
            "config skills delete",
            "file upload",
            "file download",
            "file public-url",
        ] {
            assert!(table.get(key).is_some(), "missing {key}");
        }
    }
}
