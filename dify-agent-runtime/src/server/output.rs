use crate::server::errors::ServerError;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct OutputWindow {
    pub output: String,
    pub offset: i64,
    pub truncated: bool,
}

pub fn read_output_window(path: &Path, offset: i64, limit: i64) -> Result<OutputWindow, ServerError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if offset == 0 {
                return Ok(OutputWindow {
                    output: String::new(),
                    offset: 0,
                    truncated: false,
                });
            }
            return Err(ServerError::new(
                400,
                "invalid_offset",
                "offset exceeds current file size 0",
            ));
        }
        Err(e) => return Err(ServerError::new(500, "internal_error", e.to_string())),
    };
    let size = meta.len() as i64;
    if offset > size {
        return Err(ServerError::new(
            400,
            "invalid_offset",
            format!("offset {offset} exceeds current file size {size}"),
        ));
    }
    if offset == size {
        return Ok(OutputWindow {
            output: String::new(),
            offset,
            truncated: false,
        });
    }
    let mut read_size = limit + 4;
    if offset + read_size > size {
        read_size = size - offset;
    }
    let mut f = fs::File::open(path).map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(offset as u64))
        .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
    let mut buf = vec![0u8; read_size as usize];
    let n = f
        .read(&mut buf)
        .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
    buf.truncate(n);
    let start_shift = advance_to_utf8_boundary(&buf, 0);
    let data = &buf[start_shift..];
    let mut budget = limit - start_shift as i64;
    if budget < 0 {
        budget = 0;
    }
    let mut consumed = valid_utf8_prefix_len(data, budget as usize);
    if consumed == 0 && !data.is_empty() {
        consumed = first_complete_rune_len(data);
    }
    let output_bytes = &data[..consumed];
    let new_offset = offset + start_shift as i64 + consumed as i64;
    Ok(OutputWindow {
        output: String::from_utf8_lossy(output_bytes).into_owned(),
        offset: new_offset,
        truncated: new_offset < size,
    })
}

pub fn tail_output_window(path: &Path, limit: i64) -> Result<OutputWindow, ServerError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OutputWindow {
                output: String::new(),
                offset: 0,
                truncated: false,
            });
        }
        Err(e) => return Err(ServerError::new(500, "internal_error", e.to_string())),
    };
    let size = meta.len() as i64;
    if size == 0 {
        return Ok(OutputWindow {
            output: String::new(),
            offset: 0,
            truncated: false,
        });
    }
    let mut start = size - limit;
    if start < 0 {
        start = 0;
    }
    let mut padded = start - 4;
    if padded < 0 {
        padded = 0;
    }
    let mut f = fs::File::open(path).map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(padded as u64))
        .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
    let read_len = (size - padded) as usize;
    let mut buf = vec![0u8; read_len];
    let n = f
        .read(&mut buf)
        .map_err(|e| ServerError::new(500, "internal_error", e.to_string()))?;
    buf.truncate(n);
    let relative = advance_to_utf8_boundary(&buf, (start - padded) as usize);
    let payload = &buf[relative..];
    let consumed = valid_utf8_prefix_len(payload, payload.len());
    Ok(OutputWindow {
        output: String::from_utf8_lossy(&payload[..consumed]).into_owned(),
        offset: padded + relative as i64 + consumed as i64,
        truncated: false,
    })
}

fn advance_to_utf8_boundary(data: &[u8], mut start: usize) -> usize {
    while start < data.len() && is_utf8_continuation(data[start]) {
        start += 1;
    }
    start
}

fn is_utf8_continuation(b: u8) -> bool {
    (b & 0xC0) == 0x80
}

fn valid_utf8_prefix_len(data: &[u8], max_len: usize) -> usize {
    let mut end = max_len.min(data.len());
    while end > 0 {
        if std::str::from_utf8(&data[..end]).is_ok() {
            return end;
        }
        end -= 1;
    }
    0
}

fn first_complete_rune_len(data: &[u8]) -> usize {
    for end in 1..=data.len().min(4) {
        if std::str::from_utf8(&data[..end]).is_ok() {
            return end;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn empty_missing() {
        let w = read_output_window(Path::new("/no/such"), 0, 16).unwrap();
        assert_eq!(w.offset, 0);
    }

    #[test]
    fn forward_window() {
        let d = tempdir().unwrap();
        let p = d.path().join("o.log");
        std::fs::write(&p, "hello world\n").unwrap();
        let w = read_output_window(&p, 0, 5).unwrap();
        assert_eq!(w.output, "hello");
        assert!(w.truncated);
    }
}
