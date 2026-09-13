use zip::write::SimpleFileOptions;
use std::fs::{self, File};
use std::io::{self};
use std::path::{Path, PathBuf};
use zip::{ZipArchive, ZipWriter};

pub fn create_zip_archive(archive_path: &Path, dir_path: &Path) -> Result<(), String> {
    let file = File::create(archive_path).map_err(|e| format!("create archive file: {e}"))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    walk(dir_path, dir_path, &mut writer, options)?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn walk(
    root: &Path,
    current: &Path,
    writer: &mut ZipWriter<File>,
    options: SimpleFileOptions,
) -> Result<(), String> {
    let skip_files = [".DS_Store", ".DIFY-SKILL-FULL.zip"];
    for ent in fs::read_dir(current).map_err(|e| e.to_string())? {
        let ent = ent.map_err(|e| e.to_string())?;
        let path = ent.path();
        let name_s = ent.file_name().to_string_lossy().into_owned();
        let ft = ent.file_type().map_err(|e| e.to_string())?;
        if ft.is_dir() {
            if should_skip_dir(&name_s) {
                continue;
            }
            walk(root, &path, writer, options)?;
            continue;
        }
        if skip_files.contains(&name_s.as_str()) {
            continue;
        }
        if ft.is_symlink() {
            return Err(format!(
                "archive does not support symlinked files: {}",
                path.display()
            ));
        }
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        writer.start_file(&rel, options).map_err(|e| e.to_string())?;
        let mut f = File::open(&path).map_err(|e| e.to_string())?;
        io::copy(&mut f, writer).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn extract_zip(archive_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|e| format!("open zip: {e}"))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("open zip: {e}"))?;
    fs::create_dir_all(target_dir).map_err(|e| e.to_string())?;
    let abs_target = fs::canonicalize(target_dir).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().to_string();
        let dest = abs_target.join(&name);
        let dest_norm = normalize(&dest);
        let tgt_norm = normalize(&abs_target);
        if !dest_norm.starts_with(&tgt_norm) {
            return Err(format!("archive entry escapes target directory: {name}"));
        }
        if f.is_dir() {
            fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut out = File::create(&dest).map_err(|e| e.to_string())?;
        io::copy(&mut f, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | "__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".venv" | "node_modules"
    )
}

pub fn build_skill_archive(dir_path: &Path) -> Result<PathBuf, String> {
    let archive_path = std::env::temp_dir().join(format!(
        "skill-archive-{}-{}.zip",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    if let Err(e) = create_zip_archive(&archive_path, dir_path) {
        let _ = fs::remove_file(&archive_path);
        return Err(e);
    }
    Ok(archive_path)
}
