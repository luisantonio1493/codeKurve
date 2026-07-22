//! Minimal hand-rolled argument parsing. ponytail: no `clap` while the surface
//! is this small (§12 lists clap for when the command set grows in Phase 2+).

use std::path::PathBuf;

/// Parsed positional arguments and the `--root` flag.
pub struct Args {
    pub positionals: Vec<String>,
    pub root: PathBuf,
}

impl Args {
    pub fn parse<I: Iterator<Item = String>>(args: I) -> Self {
        let mut positionals = Vec::new();
        let mut root = PathBuf::from(".");
        let mut iter = args;
        while let Some(arg) = iter.next() {
            if arg == "--root" {
                if let Some(value) = iter.next() {
                    root = PathBuf::from(value);
                }
            } else {
                positionals.push(arg);
            }
        }
        Self { positionals, root }
    }

    pub fn positional(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }
}
