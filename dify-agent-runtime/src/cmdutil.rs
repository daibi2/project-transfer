use std::io::{self, Write};
use std::process;

pub fn handle_error(err: Option<impl std::fmt::Display>, code: i32, msg: &str) {
    if let Some(e) = err {
        let _ = writeln!(io::stderr(), "shellctl: {msg}: {e}");
        process::exit(code);
    }
}

pub fn handle_result<T, E: std::fmt::Display>(result: Result<T, E>, code: i32, msg: &str) -> T {
    match result {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "shellctl: {msg}: {e}");
            process::exit(code);
        }
    }
}
