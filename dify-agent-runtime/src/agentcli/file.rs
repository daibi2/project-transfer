use super::client::new_stub_client;
use super::env::Environment;
use base64::Engine;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
pub struct FileUploadResponse {
    pub transfer_method: String,
    pub reference: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub public_download_url: String,
}

pub async fn run_file_upload(env: &Environment, path: &str, no_download_link: bool) -> Result<(), String> {
    let client = new_stub_client(env)?;
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    if !abs.is_file() {
        return Err(format!("local file not found: {}", abs.display()));
    }
    let filename = abs.file_name().unwrap().to_string_lossy().into_owned();
    let mimetype = guess_mime_type(&filename);
    let upload_url = client
        .create_tool_file_upload_url(&filename, &mimetype)
        .await
        .map_err(|e| format!("request file upload URL: {e}"))?;
    let upload_body = client
        .upload_file_to_url(&upload_url, &abs.to_string_lossy(), &filename, &mimetype)
        .await
        .map_err(|e| format!("upload file data: {e}"))?;
    let v: serde_json::Value =
        serde_json::from_slice(&upload_body).map_err(|e| format!("parse upload result: {e}"))?;
    let reference = v
        .get("reference")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if reference.is_empty() {
        return Err("signed file upload response is missing reference".into());
    }
    if !is_canonical_dify_file_reference(&reference) {
        return Err("signed file upload response has invalid reference".into());
    }
    let mut result = FileUploadResponse {
        transfer_method: "tool_file".into(),
        reference: reference.clone(),
        public_download_url: String::new(),
    };
    if !no_download_link {
        match client
            .create_file_download_url("tool_file", Some(&reference), None, true)
            .await
        {
            Ok(dl) if !dl.download_url.is_empty() => {
                result.public_download_url = dl.download_url;
            }
            Ok(_) => {
                println!("{}", serde_json::to_string(&result).unwrap());
                return Err(format!(
                    "public file download response is missing download_url; retry without uploading again: dify-agent file public-url {}",
                    shell_quote(&reference)
                ));
            }
            Err(e) => {
                println!("{}", serde_json::to_string(&result).unwrap());
                return Err(format!(
                    "request public download URL: {e}; retry without uploading again: dify-agent file public-url {}",
                    shell_quote(&reference)
                ));
            }
        }
    }
    println!("{}", serde_json::to_string(&result).unwrap());
    Ok(())
}

pub async fn run_file_public_url(env: &Environment, reference: &str) -> Result<(), String> {
    if reference.is_empty() {
        return Err("file reference must not be empty".into());
    }
    let client = new_stub_client(env)?;
    let download = client
        .create_file_download_url("tool_file", Some(reference), None, true)
        .await
        .map_err(|e| format!("request public download URL: {e}"))?;
    if download.download_url.is_empty() {
        return Err("public file download response is missing download_url".into());
    }
    let result = FileUploadResponse {
        transfer_method: "tool_file".into(),
        reference: reference.into(),
        public_download_url: download.download_url,
    };
    println!("{}", serde_json::to_string(&result).unwrap());
    Ok(())
}

pub async fn run_file_download(
    env: &Environment,
    transfer_method: &str,
    reference_or_url: &str,
    local_dir: &str,
) -> Result<(), String> {
    let (reference, url) = if transfer_method == "remote_url" {
        (None, Some(reference_or_url))
    } else {
        (Some(reference_or_url), None)
    };
    let client = new_stub_client(env)?;
    let dl = client
        .create_file_download_url(transfer_method, reference, url, false)
        .await
        .map_err(|e| format!("request file download URL: {e}"))?;
    if dl.download_url.is_empty() {
        return Err("signed file download response is missing download_url".into());
    }
    if dl.filename.is_empty() {
        return Err("signed file download response is missing filename".into());
    }
    let data = client
        .download_from_url(&dl.download_url)
        .await
        .map_err(|e| format!("download file data: {e}"))?;
    let target_dir = if local_dir.is_empty() {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    } else {
        PathBuf::from(local_dir)
    };
    std::fs::create_dir_all(&target_dir).map_err(|e| format!("create target directory: {e}"))?;
    let dest = deduplicate_path(&target_dir.join(sanitize_filename(&dl.filename)));
    std::fs::write(&dest, data).map_err(|e| format!("write file: {e}"))?;
    println!("{}", dest.display());
    Ok(())
}

pub fn guess_mime_type(filename: &str) -> String {
    let mime = mime_guess::from_path(filename).first_or_octet_stream();
    let s = mime.essence_str().to_string();
    if s.is_empty() {
        "application/octet-stream".into()
    } else {
        s
    }
}

fn sanitize_filename(filename: &str) -> String {
    let name = Path::new(filename)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." {
        "downloaded".into()
    } else {
        name
    }
}

fn deduplicate_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let stem = path.file_stem().unwrap().to_string_lossy();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut counter = 1;
    loop {
        let name = if ext.is_empty() {
            format!("{stem} ({counter})")
        } else {
            format!("{stem} ({counter}).{ext}")
        };
        let cand = dir.join(name);
        if !cand.exists() {
            return cand;
        }
        counter += 1;
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn is_canonical_dify_file_reference(reference: &str) -> bool {
    let Some(encoded) = reference.strip_prefix("dify-file-ref:") else {
        return false;
    };
    if encoded.is_empty() {
        return false;
    }
    let Ok(payload) = base64::engine::general_purpose::URL_SAFE.decode(encoded) else {
        return false;
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&payload) else {
        return false;
    };
    v.get("record_id")
        .and_then(|x| x.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}
