use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EscapeState {
    Normal,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

pub struct PtySanitizer {
    line_buffer: Vec<u8>,
    pending_cr: bool,
    state: EscapeState,
}

impl Default for PtySanitizer {
    fn default() -> Self {
        Self::new()
    }
}

impl PtySanitizer {
    pub fn new() -> Self {
        Self {
            line_buffer: Vec::new(),
            pending_cr: false,
            state: EscapeState::Normal,
        }
    }

    pub fn feed(&mut self, text: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let (ch, size) = decode_rune(rest);
            rest = &rest[size..];
            out = self.consume_rune(ch, out);
        }
        out
    }

    pub fn flush(&mut self) -> Vec<u8> {
        self.state = EscapeState::Normal;
        self.pending_cr = false;
        std::mem::take(&mut self.line_buffer)
    }

    fn consume_rune(&mut self, r: char, out: Vec<u8>) -> Vec<u8> {
        match self.state {
            EscapeState::Normal => {
                if r == '\u{1b}' {
                    self.state = EscapeState::Esc;
                    return out;
                }
                self.consume_visible(r, out)
            }
            EscapeState::Esc => {
                match r {
                    '[' => self.state = EscapeState::Csi,
                    ']' => self.state = EscapeState::Osc,
                    _ => {
                        self.state = EscapeState::Normal;
                        if r != '\u{1b}' && is_printable(r) {
                            return self.consume_visible(r, out);
                        }
                    }
                }
                out
            }
            EscapeState::Csi => {
                if ('@'..='~').contains(&r) {
                    self.state = EscapeState::Normal;
                }
                out
            }
            EscapeState::Osc => {
                match r {
                    '\u{07}' => self.state = EscapeState::Normal,
                    '\u{1b}' => self.state = EscapeState::OscEsc,
                    _ => {}
                }
                out
            }
            EscapeState::OscEsc => {
                if r == '\\' {
                    self.state = EscapeState::Normal;
                } else {
                    self.state = EscapeState::Osc;
                }
                out
            }
        }
    }

    fn consume_visible(&mut self, r: char, mut out: Vec<u8>) -> Vec<u8> {
        if self.pending_cr {
            if r == '\n' {
                out.extend_from_slice(&self.line_buffer);
                out.push(b'\n');
                self.line_buffer.clear();
                self.pending_cr = false;
                return out;
            }
            self.line_buffer.clear();
            self.pending_cr = false;
        }
        if r == '\r' {
            self.pending_cr = true;
            return out;
        }
        if r == '\n' {
            out.extend_from_slice(&self.line_buffer);
            out.push(b'\n');
            self.line_buffer.clear();
            return out;
        }
        let mut buf = [0u8; 4];
        let encoded = r.encode_utf8(&mut buf);
        self.line_buffer.extend_from_slice(encoded.as_bytes());
        out
    }
}

fn is_printable(r: char) -> bool {
    r >= '\u{20}' && r != '\u{7f}'
}

/// Decode one Unicode scalar, replacing invalid UTF-8 bytes with U+FFFD.
fn decode_rune(text: &[u8]) -> (char, usize) {
    match std::str::from_utf8(text) {
        Ok(s) => {
            let ch = s.chars().next().unwrap();
            (ch, ch.len_utf8())
        }
        Err(e) => {
            let valid = e.valid_up_to();
            if valid > 0 {
                let s = std::str::from_utf8(&text[..valid]).unwrap();
                let ch = s.chars().next().unwrap();
                return (ch, ch.len_utf8());
            }
            ('\u{FFFD}', 1)
        }
    }
}

pub fn run(ready_file: Option<&str>, stdin: impl Read, stdout: impl Write) -> io::Result<()> {
    if let Some(path) = ready_file {
        if !path.is_empty() {
            File::create(Path::new(path))?;
        }
    }
    let mut sanitizer = PtySanitizer::new();
    let mut reader = BufReader::with_capacity(65536, stdin);
    let mut writer = BufWriter::new(stdout);
    let mut buf = vec![0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n > 0 {
            let out = sanitizer.feed(&buf[..n]);
            if !out.is_empty() {
                writer.write_all(&out)?;
                writer.flush()?;
            }
        }
        if n == 0 {
            break;
        }
    }
    let tail = sanitizer.flush();
    if !tail.is_empty() {
        writer.write_all(&tail)?;
    }
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_all(input: &[u8]) -> String {
        let mut s = PtySanitizer::new();
        let mut out = s.feed(input);
        out.extend(s.flush());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn plain_text() {
        assert_eq!(run_all(b"hello\nworld\n"), "hello\nworld\n");
    }

    #[test]
    fn strip_csi() {
        assert_eq!(run_all(b"\x1b[31mred\x1b[0m\n"), "red\n");
    }

    #[test]
    fn carriage_return_overwrite() {
        assert_eq!(run_all(b"50%\r100%\n"), "100%\n");
    }

    #[test]
    fn crlf() {
        assert_eq!(run_all(b"line1\r\nline2\r\n"), "line1\nline2\n");
    }

    #[test]
    fn osc_sequence() {
        assert_eq!(run_all(b"\x1b]0;title\x07visible\n"), "visible\n");
    }

    #[test]
    fn flush_unterminated_line() {
        assert_eq!(run_all(b"no newline"), "no newline");
    }

    #[test]
    fn invalid_utf8() {
        assert_eq!(run_all(&[0xFF, b'a', b'\n']), "\u{FFFD}a\n");
    }
}
