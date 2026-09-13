mod gitignore;
mod restore;
mod save;

pub use restore::{RestoreResult, RestoreError, restore_home, ERR_MALFORMED};
pub use save::save_home;

pub const RUNTIME_STATE_DIR: &str = ".local/share/shellctl";

const DEFAULT_EXCLUDES: &[&str] = &[RUNTIME_STATE_DIR];

pub struct Excluder {
    matcher: gitignore::Matcher,
}

impl Excluder {
    pub fn new(patterns: &[String]) -> Self {
        let compiled: Vec<gitignore::Pattern> = patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| gitignore::parse_pattern(p, &[]))
            .collect();
        Self {
            matcher: gitignore::Matcher::new(compiled),
        }
    }

    pub fn excluded(&self, rel: &str, is_dir: bool) -> bool {
        if is_default_excluded(rel) {
            return true;
        }
        let parts: Vec<&str> = rel.split('/').collect();
        self.matcher.r#match(&parts, is_dir)
    }
}

fn is_default_excluded(rel: &str) -> bool {
    DEFAULT_EXCLUDES.contains(&rel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_patterns() {
        let cases: &[(&[&str], &str, bool, bool)] = &[
            (&[".cache"], ".cache", true, true),
            (&["node_modules"], "src/app/node_modules", true, true),
            (&["tags"], "src/tags", false, true),
            (&["/build"], "build", true, true),
            (&["/build"], "src/build", true, false),
            (&["*.log"], "var/app.log", false, true),
            (&["*.log"], "var/app.txt", false, false),
            (&[".cache/**"], ".cache/a/b/c.bin", false, true),
            (&["src/**/dist"], "src/a/b/dist", true, true),
            (&["target/"], "target", false, false),
            (&["target/"], "target", true, true),
            (&["tmp?"], "tmp1", false, true),
            (&["nothing-here"], "bin/tool.sh", false, false),
        ];
        for (patterns, rel, is_dir, want) in cases {
            let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
            let got = Excluder::new(&pats).excluded(rel, *is_dir);
            assert_eq!(got, *want, "Excluded({rel}, {is_dir}) with {patterns:?}");
        }
        let empty: Vec<String> = vec![];
        assert!(!Excluder::new(&empty).excluded("bin/tool.sh", false));
    }

    #[test]
    fn negation_wins() {
        let e = Excluder::new(&[".cache/**".into(), "!.cache/keep-me".into()]);
        assert!(e.excluded(".cache/junk", false));
        assert!(!e.excluded(".cache/keep-me", false));
    }

    #[test]
    fn defaults_beat_caller() {
        for pattern in [
            format!("!{RUNTIME_STATE_DIR}"),
            format!("!/{RUNTIME_STATE_DIR}"),
            format!("!{RUNTIME_STATE_DIR}/**"),
            "!**".into(),
        ] {
            let e = Excluder::new(&[pattern.clone()]);
            for dir in DEFAULT_EXCLUDES {
                assert!(e.excluded(dir, true), "{pattern} pulled {dir} back in");
            }
        }
    }

    #[test]
    fn ignores_blanks() {
        let e = Excluder::new(&["".into(), "   ".into(), "\t".into()]);
        assert!(!e.excluded("bin/tool.sh", false));
    }
}
