use super::Excluder;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tar::{Builder, Header};
use zstd::stream::write::Encoder;

pub fn save_home<W: Write>(dst: W, home_dir: &Path, excludes: &[String]) -> io::Result<()> {
    let excluder = Excluder::new(excludes);
    let mut encoder = Encoder::new(dst, 3)?;
    let mut builder = Builder::new(&mut encoder);
    visit(home_dir, home_dir, &excluder, &mut builder)?;
    builder.finish()?;
    drop(builder);
    encoder.finish()?;
    Ok(())
}

fn visit<W: Write>(
    home: &Path,
    current: &Path,
    excluder: &Excluder,
    builder: &mut Builder<W>,
) -> io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(current)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.file_name());
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for ent in entries {
        let path = ent.path();
        let rel = match path.strip_prefix(home) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_slash = rel.to_string_lossy().replace('\\', "/");
        if rel_slash.is_empty() {
            continue;
        }
        let ft = ent.file_type()?;
        if excluder.excluded(&rel_slash, ft.is_dir()) {
            continue;
        }
        if ft.is_symlink() {
            let target = fs::read_link(&path)?;
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(0o777);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            builder.append_link(&mut header, &rel_slash, target)?;
        } else if ft.is_dir() {
            let meta = fs::metadata(&path)?;
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(unix_mode(&meta));
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            builder.append_data(&mut header, format!("{rel_slash}/"), io::empty())?;
            subdirs.push(path);
        } else if ft.is_file() {
            let meta = fs::metadata(&path)?;
            let mut header = Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(meta.len());
            header.set_mode(unix_mode(&meta));
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_cksum();
            let mut f = File::open(&path)?;
            builder.append_data(&mut header, &rel_slash, &mut f)?;
        }
    }
    for d in subdirs {
        visit(home, &d, excluder, builder)?;
    }
    Ok(())
}

fn unix_mode(meta: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        0o644
    }
}
