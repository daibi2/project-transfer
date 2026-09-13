use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Pty,
    Stdio,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Pty => "pty",
            Mode::Stdio => "stdio",
        }
    }
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn parse(raw: &str) -> Result<Mode, String> {
    if raw.is_empty() {
        return Ok(Mode::Pty);
    }
    match raw {
        "pty" => Ok(Mode::Pty),
        "stdio" => Ok(Mode::Stdio),
        other => Err(format!("invalid job mode {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode() {
        assert_eq!(parse("").unwrap(), Mode::Pty);
        assert_eq!(parse("pty").unwrap(), Mode::Pty);
        assert_eq!(parse("stdio").unwrap(), Mode::Stdio);
        assert!(parse("stdout").is_err());
    }
}
