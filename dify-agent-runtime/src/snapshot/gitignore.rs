//! Vendored gitignore matcher (go-git plumbing/format/gitignore), ported to Rust.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MatchResult {
    NoMatch,
    Exclude,
    Include,
}

const INCLUSION_PREFIX: &str = "!";
const ZERO_TO_MANY_DIRS: &str = "**";
const PATTERN_DIR_SEP: &str = "/";

#[derive(Clone)]
pub struct Pattern {
    domain: Vec<String>,
    pattern: Vec<String>,
    inclusion: bool,
    dir_only: bool,
    is_glob: bool,
}

pub fn parse_pattern(p: &str, domain: &[String]) -> Pattern {
    let mut p = p.to_string();
    let mut res = Pattern {
        domain: domain.to_vec(),
        pattern: Vec::new(),
        inclusion: false,
        dir_only: false,
        is_glob: false,
    };
    if let Some(rest) = p.strip_prefix(INCLUSION_PREFIX) {
        res.inclusion = true;
        p = rest.to_string();
    }
    if !p.ends_with("\\ ") {
        p = p.trim_end_matches(' ').to_string();
    }
    if p.ends_with(PATTERN_DIR_SEP) {
        res.dir_only = true;
        p.pop();
    }
    if p.contains(PATTERN_DIR_SEP) {
        res.is_glob = true;
    }
    res.pattern = p.split(PATTERN_DIR_SEP).map(|s| s.to_string()).collect();
    res
}

impl Pattern {
    pub fn r#match(&self, path: &[&str], is_dir: bool) -> MatchResult {
        if path.len() <= self.domain.len() {
            return MatchResult::NoMatch;
        }
        for (i, e) in self.domain.iter().enumerate() {
            if path[i] != e {
                return MatchResult::NoMatch;
            }
        }
        let path = &path[self.domain.len()..];
        let ok = if self.is_glob {
            self.glob_match(path, is_dir)
        } else {
            self.simple_name_match(path, is_dir)
        };
        if !ok {
            return MatchResult::NoMatch;
        }
        if self.inclusion {
            MatchResult::Include
        } else {
            MatchResult::Exclude
        }
    }

    fn simple_name_match(&self, path: &[&str], is_dir: bool) -> bool {
        let pat = &self.pattern[0];
        for (i, name) in path.iter().enumerate() {
            if !filepath_match(pat, name) {
                continue;
            }
            if self.dir_only && !is_dir && i == path.len() - 1 {
                return false;
            }
            return true;
        }
        false
    }

    fn glob_match(&self, mut path: &[&str], is_dir: bool) -> bool {
        let mut matched = false;
        let mut can_traverse = false;
        let mut i = 0;
        while i < self.pattern.len() {
            let pattern = &self.pattern[i];
            if pattern.is_empty() {
                can_traverse = false;
                i += 1;
                continue;
            }
            if pattern == ZERO_TO_MANY_DIRS {
                if i == self.pattern.len() - 1 {
                    break;
                }
                can_traverse = true;
                i += 1;
                continue;
            }
            if pattern.contains(ZERO_TO_MANY_DIRS) {
                return false;
            }
            if path.is_empty() {
                return false;
            }
            if can_traverse {
                can_traverse = false;
                let mut found = false;
                while !path.is_empty() {
                    let e = path[0];
                    path = &path[1..];
                    if filepath_match(pattern, e) {
                        matched = true;
                        found = true;
                        break;
                    } else if path.is_empty() {
                        matched = false;
                    }
                }
                if !found && path.is_empty() {
                    // fallthrough; matched may be false
                }
            } else {
                if !filepath_match(pattern, path[0]) {
                    return false;
                }
                matched = true;
                path = &path[1..];
            }
            i += 1;
        }
        if matched && self.dir_only && !is_dir && path.is_empty() {
            matched = false;
        }
        matched
    }
}

pub struct Matcher {
    patterns: Vec<Pattern>,
}

impl Matcher {
    pub fn new(patterns: Vec<Pattern>) -> Self {
        Self { patterns }
    }

    pub fn r#match(&self, path: &[&str], is_dir: bool) -> bool {
        for p in self.patterns.iter().rev() {
            let m = p.r#match(path, is_dir);
            if m != MatchResult::NoMatch {
                return m == MatchResult::Exclude;
            }
        }
        false
    }
}

/// Go `path/filepath.Match` for a single name (no separators in `name`).
pub fn filepath_match(pattern: &str, name: &str) -> bool {
    match_go(pattern.as_bytes(), name.as_bytes())
}

fn match_go(pattern: &[u8], name: &[u8]) -> bool {
    let mut px = 0;
    let mut nx = 0;
    while px < pattern.len() || nx < name.len() {
        if px < pattern.len() {
            match pattern[px] {
                b'*' => {
                    let rest = &pattern[px + 1..];
                    if rest.is_empty() {
                        return !name[nx..].contains(&b'/');
                    }
                    for skip in 0..=name.len() - nx {
                        if name.get(nx + skip) == Some(&b'/') {
                            return false;
                        }
                        if match_go(rest, &name[nx + skip..]) {
                            return true;
                        }
                    }
                    return false;
                }
                b'?' => {
                    if nx < name.len() && name[nx] != b'/' {
                        px += 1;
                        nx += 1;
                        continue;
                    }
                    return false;
                }
                b'[' => {
                    if nx >= name.len() {
                        return false;
                    }
                    match scan_class(&pattern[px..], name[nx]) {
                        Some((consumed, true)) => {
                            px += consumed;
                            nx += 1;
                            continue;
                        }
                        _ => return false,
                    }
                }
                b'\\' => {
                    px += 1;
                    if px >= pattern.len() {
                        return false;
                    }
                    if nx < name.len() && name[nx] == pattern[px] {
                        px += 1;
                        nx += 1;
                        continue;
                    }
                    return false;
                }
                c => {
                    if nx < name.len() && name[nx] == c {
                        px += 1;
                        nx += 1;
                        continue;
                    }
                    return false;
                }
            }
        } else {
            return false;
        }
    }
    true
}

fn scan_class(pattern: &[u8], ch: u8) -> Option<(usize, bool)> {
    // pattern starts at '['
    if pattern.len() < 2 {
        return None;
    }
    let mut i = 1;
    let mut negated = false;
    if pattern.get(i) == Some(&b'^') {
        negated = true;
        i += 1;
    }
    let mut matched = false;
    while i < pattern.len() && pattern[i] != b']' {
        if i + 2 < pattern.len() && pattern[i + 1] == b'-' && pattern[i + 2] != b']' {
            let lo = pattern[i];
            let hi = pattern[i + 2];
            if ch >= lo && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if pattern[i] == ch {
                matched = true;
            }
            i += 1;
        }
    }
    if i >= pattern.len() || pattern[i] != b']' {
        return None;
    }
    Some((i + 1, matched != negated))
}
