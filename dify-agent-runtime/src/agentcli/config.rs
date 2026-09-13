use super::archive::{build_skill_archive, extract_zip};
use super::client::new_stub_client;
use super::env::Environment;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const DEFAULT_CONFIG_BASE: &str = ".dify_conf";

#[derive(serde::Serialize)]
struct ConfigFileRef {
    kind: String,
    id: String,
}

pub async fn run_config_manifest(env: &Environment) -> Result<(), String> {
    let client = new_stub_client(env)?;
    let body = client.get_config_manifest().await?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

pub async fn run_config_skills_pull(
    env: &Environment,
    mut names: Vec<String>,
    local_dir: String,
    json_output: bool,
) -> Result<(), String> {
    let client = new_stub_client(env)?;
    if names.is_empty() {
        let body = client.get_config_manifest().await?;
        let v: Value = serde_json::from_slice(&body).map_err(|e| format!("parse manifest: {e}"))?;
        if let Some(items) = v.pointer("/skills/items").and_then(|x| x.as_array()) {
            for item in items {
                if let Some(n) = item.get("name").and_then(|x| x.as_str()) {
                    names.push(n.to_string());
                }
            }
        }
    }
    let target_dir = if local_dir.is_empty() {
        PathBuf::from(DEFAULT_CONFIG_BASE).join("skills")
    } else {
        PathBuf::from(local_dir)
    };
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("create target directory: {e}"))?;
    let mut items = Vec::new();
    for name in &names {
        let download = client
            .create_config_download_url("skill", name)
            .await
            .map_err(|e| format!("request config skill {name:?} download URL: {e}"))?;
        let archive_bytes = client
            .download_from_url(&download.download_url)
            .await
            .map_err(|e| format!("download config skill {name:?}: {e}"))?;
        let archive_path = target_dir.join(format!("{name}.zip"));
        let skill_dir = target_dir.join(name);
        std::fs::create_dir_all(&skill_dir).map_err(|e| format!("create skill directory: {e}"))?;
        std::fs::write(&archive_path, &archive_bytes).map_err(|e| format!("write archive: {e}"))?;
        extract_zip(&archive_path, &skill_dir).map_err(|e| format!("extract skill archive: {e}"))?;
        let skill_md = std::fs::read_to_string(skill_dir.join("SKILL.md")).unwrap_or_default();
        items.push(json!({
            "name": name,
            "archive_path": archive_path,
            "directory_path": skill_dir,
            "skill_md": skill_md,
        }));
    }
    if json_output {
        println!("{}", json!({"items": items}));
        return Ok(());
    }
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            println!();
        }
        println!("{}", item["directory_path"].as_str().unwrap_or(""));
        print!("{}", item["skill_md"].as_str().unwrap_or(""));
    }
    Ok(())
}

pub async fn run_config_files_pull(
    env: &Environment,
    mut names: Vec<String>,
    local_dir: String,
    json_output: bool,
) -> Result<(), String> {
    let client = new_stub_client(env)?;
    if names.is_empty() {
        let body = client.get_config_manifest().await?;
        let v: Value = serde_json::from_slice(&body).map_err(|e| format!("parse manifest: {e}"))?;
        if let Some(items) = v.pointer("/files/items").and_then(|x| x.as_array()) {
            for item in items {
                if let Some(n) = item.get("name").and_then(|x| x.as_str()) {
                    names.push(n.to_string());
                }
            }
        }
    }
    let target_dir = if local_dir.is_empty() {
        PathBuf::from(DEFAULT_CONFIG_BASE).join("files")
    } else {
        PathBuf::from(local_dir)
    };
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("create target directory: {e}"))?;
    let mut items = Vec::new();
    for name in &names {
        let download = client
            .create_config_download_url("file", name)
            .await
            .map_err(|e| format!("request config file {name:?} download URL: {e}"))?;
        let payload = client
            .download_from_url(&download.download_url)
            .await
            .map_err(|e| format!("download config file {name:?}: {e}"))?;
        let target_path = target_dir.join(name);
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create parent directory: {e}"))?;
        }
        std::fs::write(&target_path, payload).map_err(|e| format!("write file: {e}"))?;
        items.push(json!({"name": name, "path": target_path}));
    }
    if json_output {
        println!("{}", json!({"items": items}));
        return Ok(());
    }
    for item in items {
        println!("{}", item["path"].as_str().unwrap_or(""));
    }
    Ok(())
}

pub async fn run_config_skills_push(env: &Environment, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("at least one skill directory is required".into());
    }
    let client = new_stub_client(env)?;
    let mut skills = Vec::new();
    for path in paths {
        let abs = std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
        if !abs.is_dir() {
            return Err(format!("config skill path must be a directory: {}", abs.display()));
        }
        if !abs.join("SKILL.md").exists() {
            return Err(format!(
                "config skill directory must contain SKILL.md: {}",
                abs.display()
            ));
        }
        let archive_path = build_skill_archive(&abs)?;
        let name = abs.file_name().unwrap().to_string_lossy().into_owned();
        let file_ref = upload_config_file(&client, &archive_path)
            .await
            .map_err(|e| format!("upload config skill {name:?}: {e}"))?;
        let _ = std::fs::remove_file(archive_path);
        skills.push(json!({"name": name, "file_ref": file_ref}));
    }
    let body = client
        .push_config(&json!({"skills": skills, "files": []}))
        .await
        .map_err(|e| format!("push config skills: {e}"))?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

pub async fn run_config_files_push(env: &Environment, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() {
        return Err("at least one file path is required".into());
    }
    let client = new_stub_client(env)?;
    let mut files = Vec::new();
    for path in paths {
        let abs = std::fs::canonicalize(&path).unwrap_or_else(|_| PathBuf::from(&path));
        if abs.is_dir() || !abs.is_file() {
            return Err(format!(
                "config file path must be a regular file: {}",
                abs.display()
            ));
        }
        let name = abs.file_name().unwrap().to_string_lossy().into_owned();
        let file_ref = upload_config_file(&client, &abs)
            .await
            .map_err(|e| format!("upload config file {name:?}: {e}"))?;
        files.push(json!({"name": name, "file_ref": file_ref}));
    }
    let body = client
        .push_config(&json!({"files": files, "skills": []}))
        .await
        .map_err(|e| format!("push config files: {e}"))?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

async fn upload_config_file(
    client: &super::client::StubClient,
    file_path: &Path,
) -> Result<ConfigFileRef, String> {
    let filename = file_path.file_name().unwrap().to_string_lossy().into_owned();
    let mimetype = super::file::guess_mime_type(&filename);
    let upload_url = client
        .create_tool_file_upload_url(&filename, &mimetype)
        .await
        .map_err(|e| format!("request upload URL: {e}"))?;
    let upload_body = client
        .upload_file_to_url(
            &upload_url,
            &file_path.to_string_lossy(),
            &filename,
            &mimetype,
        )
        .await
        .map_err(|e| format!("upload data: {e}"))?;
    let v: Value = serde_json::from_slice(&upload_body).map_err(|e| format!("parse upload result: {e}"))?;
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
    if id.is_empty() {
        return Err("upload response is missing id".into());
    }
    Ok(ConfigFileRef {
        kind: "tool_file".into(),
        id: id.into(),
    })
}

pub async fn run_config_skills_delete(env: &Environment, names: Vec<String>) -> Result<(), String> {
    if names.is_empty() {
        return Err("at least one skill name is required".into());
    }
    let skills: Vec<Value> = names
        .into_iter()
        .map(|n| json!({"name": n, "file_ref": null}))
        .collect();
    let client = new_stub_client(env)?;
    let body = client
        .push_config(&json!({"skills": skills, "files": []}))
        .await?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

pub async fn run_config_files_delete(env: &Environment, names: Vec<String>) -> Result<(), String> {
    if names.is_empty() {
        return Err("at least one file name is required".into());
    }
    let files: Vec<Value> = names
        .into_iter()
        .map(|n| json!({"name": n, "file_ref": null}))
        .collect();
    let client = new_stub_client(env)?;
    let body = client
        .push_config(&json!({"files": files, "skills": []}))
        .await?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

pub async fn run_config_env_push(env: &Environment, local_path: String) -> Result<(), String> {
    let env_text = if local_path == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("read stdin: {e}"))?;
        buf
    } else {
        let abs = std::fs::canonicalize(&local_path).unwrap_or_else(|_| PathBuf::from(&local_path));
        std::fs::read_to_string(abs).map_err(|e| format!("read file: {e}"))?
    };
    let client = new_stub_client(env)?;
    let body = client.patch_config_env(&env_text).await?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}

pub async fn run_config_note_pull(env: &Environment, local_path: String) -> Result<(), String> {
    let client = new_stub_client(env)?;
    let body = client.get_config_manifest().await?;
    let v: Value = serde_json::from_slice(&body).map_err(|e| format!("parse manifest: {e}"))?;
    let note = v.get("note").and_then(|x| x.as_str()).unwrap_or("");
    let target = if local_path.is_empty() {
        PathBuf::from(DEFAULT_CONFIG_BASE).join("note.md")
    } else {
        PathBuf::from(local_path)
    };
    let abs = std::env::current_dir()
        .unwrap_or_default()
        .join(&target);
    if let Some(p) = abs.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("create directory: {e}"))?;
    }
    std::fs::write(&abs, note).map_err(|e| format!("write note: {e}"))?;
    println!("{}", abs.display());
    Ok(())
}

pub async fn run_config_note_push(env: &Environment, local_path: String) -> Result<(), String> {
    let note = if local_path == "-" {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("read stdin: {e}"))?;
        buf
    } else {
        let target = if local_path.is_empty() {
            PathBuf::from(DEFAULT_CONFIG_BASE).join("note.md")
        } else {
            PathBuf::from(local_path)
        };
        let abs = std::fs::canonicalize(&target).unwrap_or(target);
        std::fs::read_to_string(abs).map_err(|e| format!("read file: {e}"))?
    };
    let client = new_stub_client(env)?;
    let body = client.put_config_note(&note).await?;
    println!("{}", String::from_utf8_lossy(&body));
    Ok(())
}
