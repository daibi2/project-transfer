use rand::RngCore;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn generate_job_id() -> String {
    let mut b = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub fn format_timestamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    crate::runner::format_unix_for_ts(secs)
}

pub fn parse_timestamp(s: &str) -> Result<SystemTime, ()> {
    // 2006-01-02T15:04:05Z
    if s.len() != 20 || !s.ends_with('Z') {
        return Err(());
    }
    let y: i32 = s[0..4].parse().map_err(|_| ())?;
    let m: u32 = s[5..7].parse().map_err(|_| ())?;
    let d: u32 = s[8..10].parse().map_err(|_| ())?;
    let hh: u32 = s[11..13].parse().map_err(|_| ())?;
    let mm: u32 = s[14..16].parse().map_err(|_| ())?;
    let ss: u32 = s[17..19].parse().map_err(|_| ())?;
    let days = days_from_civil(y, m, d);
    let unix = days * 86400 + hh as i64 * 3600 + mm as i64 * 60 + ss as i64;
    Ok(UNIX_EPOCH + std::time::Duration::from_secs(unix as u64))
}

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let mut y = y as i64;
    let m = m as i64;
    let d = d as i64;
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
    (era * 146097 + doe as i64 - 719468) as i64
}

pub fn job_session_name(job_id: &str) -> String {
    format!("shellctl-{job_id}")
}

pub fn job_pane_target(job_id: &str) -> String {
    format!("{}:0.0", job_session_name(job_id))
}
