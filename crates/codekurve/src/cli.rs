//! Minimal hand-rolled argument parsing. ponytail: no `clap` while the surface
//! is this small (§12 lists clap for when the command set grows in Phase 2+).

use std::path::PathBuf;

/// Parsed positional arguments and flags. Pre-PR5b commands only ever read
/// `positionals`/`root`; the graph-query commands (§27.2) additionally read
/// `min_confidence`/`json`/`symbol_id`/`symbol_name`/`limit`/`offset`, and
/// `trace`/`impact` (PR5b-2) also read `depth`.
pub struct Args {
    pub positionals: Vec<String>,
    pub root: PathBuf,
    pub depth: Option<u32>,
    pub min_confidence: Option<String>,
    pub json: bool,
    pub symbol_id: Option<String>,
    pub symbol_name: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

impl Args {
    pub fn parse<I: Iterator<Item = String>>(args: I) -> Self {
        let mut positionals = Vec::new();
        let mut root = PathBuf::from(".");
        let mut depth = None;
        let mut min_confidence = None;
        let mut json = false;
        let mut symbol_id = None;
        let mut symbol_name = None;
        let mut limit = None;
        let mut offset = None;
        let mut iter = args;
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--root" => root = iter.next().map_or(root, PathBuf::from),
                "--depth" => depth = iter.next().and_then(|v| v.parse().ok()),
                "--min-confidence" => min_confidence = iter.next(),
                "--json" => json = true,
                "--symbol-id" => symbol_id = iter.next(),
                "--symbol-name" => symbol_name = iter.next(),
                "--limit" => limit = iter.next().and_then(|v| v.parse().ok()),
                "--offset" => offset = iter.next().and_then(|v| v.parse().ok()),
                _ => positionals.push(arg),
            }
        }
        Self {
            positionals,
            root,
            depth,
            min_confidence,
            json,
            symbol_id,
            symbol_name,
            limit,
            offset,
        }
    }

    pub fn positional(&self, index: usize) -> Option<&str> {
        self.positionals.get(index).map(String::as_str)
    }
}
