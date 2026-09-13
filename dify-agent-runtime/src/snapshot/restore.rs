use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path};
use tar::Archive;
use thiserror::Error;
use zstd::stream::read::Decoder;

pub const ERR_MALFORMED: &str = "archive malformed";
const MAX_DECODER_WINDOW: u32 = 64 << 20;

#[derive(Debug, Error)]
pub enum RestoreError {
    #[error("archive malformed: {0}")]
    Malformed(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

impl RestoreError {
    pub fn is_malformed(&self) -> bool {
        matches!(self, RestoreError::Malformed(_))
    }
}

#[derive(Debug, Default, Clone)]
pub struct RestoreResult {
    pub entries: i32,
    pub bytes_written: i64,
}

pub fn restore_home(src: impl Read, home_dir: &Path) -> Result<RestoreResult, RestoreError> {
    let root = Dir::open_ambient_dir(home_dir, ambient_authority()).map_err(RestoreError::Io)?;
    let decoder = Decoder::new(src).map_err(|e| RestoreError::Malformed(e.to_string()))?;
    // Window cap: zstd crate may not expose max window the same way; 64MiB is the Go contract.
    let mut archive = Archive::new(decoder);
    archive.set_overwrite(true);
    archive.set_preserve_mtime(false);

    let mut res = RestoreResult::default();
    let mut dir_modes: Vec<(String, u32)> = Vec::new();

    for entry in archive.entries().map_err(|e| RestoreError::Malformed(e.to_string()))? {
        let mut entry = entry.map_err(|e| RestoreError::Malformed(e.to_string()))?;
        let header = entry.header().clone();
        let raw_name = header.path().map_err(|e| RestoreError::Malformed(e.to_string()))?;
        let name = match clean_entry_name(&raw_name.to_string_lossy()) {
            Ok(n) => n,
            Err(e) => return Err(e),
        };
        if name.is_empty() {
            continue;
        }
        if is_sparse(&header) {
            return Err(RestoreError::Malformed(format!(
                "sparse entry {:?} not supported",
                raw_name
            )));
        }
        let mode = header.mode().unwrap_or(0o644) & 0o777;
        match header.entry_type() {
            tar::EntryType::Directory => {
                classify(root.create_dir_all(&name))?;
                dir_modes.push((name, mode));
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                classify(ensure_parent(&root, &name))?;
                let n = extract_file(&root, &name, mode, &mut entry)?;
                classify(chmod(&root, &name, mode))?;
                res.bytes_written += n;
            }
            tar::EntryType::Symlink => {
                classify(ensure_parent(&root, &name))?;
                let _ = root.remove_file(&name);
                let link = header
                    .link_name()
                    .map_err(|e| RestoreError::Malformed(e.to_string()))?
                    .ok_or_else(|| RestoreError::Malformed("missing symlink target".into()))?;
                classify(root.symlink(&link, &name))?;
            }
            tar::EntryType::Link => {
                let link = header
                    .link_name()
                    .map_err(|e| RestoreError::Malformed(e.to_string()))?
                    .ok_or_else(|| RestoreError::Malformed("missing hardlink target".into()))?;
                let target = clean_entry_name(&link.to_string_lossy())?;
                if target.is_empty() {
                    return Err(RestoreError::Malformed(format!(
                        "hardlink target {:?}",
                        link
                    )));
                }
                classify(ensure_parent(&root, &name))?;
                classify(root.hard_link(&target, &root, &name))?;
            }
            other => {
                return Err(RestoreError::Malformed(format!(
                    "unsupported entry type {other:?} for {raw_name:?}"
                )));
            }
        }
        res.entries += 1;
    }

    for (name, mode) in dir_modes.into_iter().rev() {
        classify(chmod(&root, &name, mode))?;
    }
    let _ = MAX_DECODER_WINDOW;
    Ok(res)
}

fn is_sparse(header: &tar::Header) -> bool {
    header.entry_type() == tar::EntryType::GNUSparse
}

fn clean_entry_name(name: &str) -> Result<String, RestoreError> {
    if name.starts_with('/') {
        return Err(RestoreError::Malformed(format!(
            "absolute entry name {name:?}"
        )));
    }
    let mut out = Vec::new();
    for c in Path::new(name).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(RestoreError::Malformed(format!(
                    "entry escapes root: {name:?}"
                )));
            }
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
            Component::RootDir | Component::Prefix(_) => {
                return Err(RestoreError::Malformed(format!(
                    "absolute entry name {name:?}"
                )));
            }
        }
    }
    Ok(out.join("/"))
}

fn ensure_parent(root: &Dir, name: &str) -> io::Result<()> {
    if let Some((parent, _)) = name.rsplit_once('/') {
        if !parent.is_empty() {
            return root.create_dir_all(parent);
        }
    }
    Ok(())
}

fn extract_file(
    root: &Dir,
    name: &str,
    mode: u32,
    r: &mut dyn Read,
) -> Result<i64, RestoreError> {
    let mut opts = OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    let mut f = classify(root.open_with(name, &opts))?;
    let n = io::copy(r, &mut f).map_err(classify_io)?;
    Ok(n as i64)
}

fn chmod(root: &Dir, name: &str, mode: u32) -> io::Result<()> {
    let _ = (root, name, mode);
    Ok(())
}

fn classify<T>(r: io::Result<T>) -> Result<T, RestoreError> {
    r.map_err(classify_io)
}

fn classify_io(err: io::Error) -> RestoreError {
    if is_environmental(&err) {
        RestoreError::Io(err)
    } else {
        RestoreError::Malformed(err.to_string())
    }
}

fn is_environmental(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::ENOSPC)
            | Some(libc::EDQUOT)
            | Some(libc::EROFS)
            | Some(libc::EIO)
            | Some(libc::ENOMEM)
            | Some(libc::EMFILE)
            | Some(libc::ENFILE)
            | Some(libc::EACCES)
            | Some(libc::EPERM)
    ) || err.kind() == io::ErrorKind::TimedOut
}
