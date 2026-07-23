//! Whole-project resolution (design §Interfaces, plan §20.2/§20.4): turns
//! the by-name/module-specifier `EdgeTarget::Unresolved` edges every
//! `extract::analyze` call leaves behind into `EdgeTarget::Global`/`External`
//! edges (or `UnresolvedReference` rows, never silently dropped — §18.3),
//! once every file in the run has been parsed. Pure library, no filesystem
//! I/O: it only ever looks at the `FileAnalysis.file` paths already produced
//! by discovery + `extract::analyze`, so no project-root parameter is
//! needed here (PR4b's pipeline is what makes that file set complete).
//!
//! This slice (PR4a-1) ships the whole-project symbol table and module
//! resolution (§20.2). Reference/call resolution against that table (§20.4)
//! is PR4a-2, which wires `SymbolTable`'s fields and `resolve_module` into a
//! `pub fn resolve()` entry point — until then nothing outside this module's
//! own tests calls them (matches this slice's rollback boundary: delete the
//! file, nothing else depends on it yet), hence the blanket dead-code
//! allowance below rather than scattering `#[allow]` per item.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use codekurve_core::{RelationshipKind, SymbolKind};

use crate::ir::{EdgeTarget, FileAnalysis};

/// Minimal `tsconfig.json` `compilerOptions.paths` alias map: prefix (with a
/// single trailing `*`) -> replacement prefix. Deliberately narrow scope
/// (design's "minimal scope" note) — no `baseUrl` chains, no mid-segment
/// wildcards, no exact (non-wildcard) entries beyond a literal match.
pub type TsconfigAliases = HashMap<String, String>;

/// A project symbol as seen from resolution: enough to build an
/// `EdgeTarget::Global` and to apply a `RelationshipKind`/`SymbolKind`
/// compatibility check (PR4a-2).
#[derive(Debug, Clone)]
pub(crate) struct ProjectSymbol {
    pub(crate) file: String,
    pub(crate) qualified_name: String,
    pub(crate) kind: SymbolKind,
}

/// Whole-project symbol index built from every file's `FileAnalysis`.
pub struct SymbolTable {
    /// Bare name -> every symbol in the project sharing it, for by-name
    /// `Calls`/`Constructs`/`Inherits`/`Implements` resolution (PR4a-2).
    pub(crate) by_name: HashMap<String, Vec<ProjectSymbol>>,
    /// file -> exported name -> symbol, for import/re-export resolution
    /// (PR4a-2).
    pub(crate) exports: HashMap<String, HashMap<String, ProjectSymbol>>,
}

impl SymbolTable {
    pub fn build(files: &[FileAnalysis]) -> Self {
        let mut by_name: HashMap<String, Vec<ProjectSymbol>> = HashMap::new();
        for file in files {
            for sym in &file.symbols {
                by_name
                    .entry(sym.name.clone())
                    .or_default()
                    .push(ProjectSymbol {
                        file: file.file.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind,
                    });
            }
        }

        let mut exports: HashMap<String, HashMap<String, ProjectSymbol>> = HashMap::new();
        for file in files {
            let by_local_key: HashMap<&str, &crate::ir::ExtractedSymbol> = file
                .symbols
                .iter()
                .map(|s| (s.local_key.as_str(), s))
                .collect();
            for rel in &file.relationships {
                if rel.kind != RelationshipKind::Exports {
                    continue;
                }
                let EdgeTarget::Local(key) = &rel.target else {
                    continue;
                };
                let Some(sym) = by_local_key.get(key.as_str()) else {
                    continue;
                };
                exports.entry(file.file.clone()).or_default().insert(
                    sym.name.clone(),
                    ProjectSymbol {
                        file: file.file.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind,
                    },
                );
            }
        }
        // Fallback: `extract.rs` only emits an `Exports` edge for named,
        // default, and re-export forms — a direct declaration export
        // (`export class Foo {}`) leaves no edge at all (see extract.rs's
        // `collect_exports` doc comment), yet that's the most common
        // real-world export style. Without this, ordinary `import { x }
        // from './mod'` against a plain `export function x() {}` would
        // never resolve. Register every top-level Class/Function/Interface
        // symbol under its own name too, when the (authoritative) loop
        // above didn't already claim that name — `is_exported` isn't
        // tracked yet (extract.rs, PR1), so this may register a
        // not-actually-exported top-level symbol; accepted MVP
        // over-inclusion, not a false *identity* match.
        for file in files {
            for sym in &file.symbols {
                if sym.parent.is_none()
                    && matches!(
                        sym.kind,
                        SymbolKind::Class | SymbolKind::Function | SymbolKind::Interface
                    )
                {
                    exports
                        .entry(file.file.clone())
                        .or_default()
                        .entry(sym.name.clone())
                        .or_insert_with(|| ProjectSymbol {
                            file: file.file.clone(),
                            qualified_name: sym.qualified_name.clone(),
                            kind: sym.kind,
                        });
                }
            }
        }

        Self { by_name, exports }
    }
}

/// Where a relative/aliased import specifier resolved to (§20.2 order).
pub(crate) enum ModuleResolution {
    Project(String),
    External(String),
    Unresolved,
}

/// §20.2: relative path -> exact file -> implicit `.ts/.tsx/.js/.jsx` ->
/// `index.*` -> tsconfig alias -> external node (never indexed).
pub(crate) fn resolve_module(
    importer: &str,
    specifier: &str,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
) -> ModuleResolution {
    if specifier.starts_with("./") || specifier.starts_with("../") {
        let candidate = join_relative(importer, specifier);
        return match find_file(&candidate, known_files) {
            Some(path) => ModuleResolution::Project(path),
            None => ModuleResolution::Unresolved,
        };
    }
    if let Some(candidate) = apply_alias(specifier, aliases) {
        return match find_file(&candidate, known_files) {
            Some(path) => ModuleResolution::Project(path),
            None => ModuleResolution::Unresolved,
        };
    }
    ModuleResolution::External(specifier.to_string())
}

/// Joins a relative specifier against the importer's directory, resolving
/// `.`/`..` segments. Forward-slash only (relative paths in this IR are
/// always `/`-separated, see `extract::qualified_name`), so this stays a
/// plain string operation rather than `std::path::Path` (which is
/// platform-separator-sensitive).
fn join_relative(importer: &str, specifier: &str) -> String {
    let mut parts: Vec<&str> = importer
        .rsplit_once('/')
        .map(|(dir, _)| dir.split('/').collect())
        .unwrap_or_default();
    for seg in specifier.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Tries `base` as-is, then with each implicit extension, then as an
/// `index.*` directory entry, per §20.2's order.
fn find_file(base: &str, known_files: &HashSet<String>) -> Option<String> {
    if known_files.contains(base) {
        return Some(base.to_string());
    }
    for ext in [".ts", ".tsx", ".js", ".jsx"] {
        let candidate = format!("{base}{ext}");
        if known_files.contains(&candidate) {
            return Some(candidate);
        }
    }
    for index in ["index.ts", "index.tsx", "index.js", "index.jsx"] {
        let candidate = if base.is_empty() {
            index.to_string()
        } else {
            format!("{base}/{index}")
        };
        if known_files.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// A single-`*`-prefix `compilerOptions.paths` alias mapping (e.g.
/// `"@app/*": "src/*"`), or a literal (no-wildcard) exact match.
fn apply_alias(specifier: &str, aliases: &TsconfigAliases) -> Option<String> {
    for (pattern, replacement) in aliases {
        match pattern.strip_suffix('*') {
            Some(prefix) => {
                if let Some(rest) = specifier.strip_prefix(prefix) {
                    let repl_prefix = replacement.strip_suffix('*').unwrap_or(replacement);
                    return Some(format!("{repl_prefix}{rest}"));
                }
            }
            None if pattern == specifier => return Some(replacement.clone()),
            None => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::analyze;
    use codekurve_core::LanguageId;

    fn analyzed(path: &str, source: &str) -> FileAnalysis {
        analyze(source, LanguageId::TypeScript, path).unwrap()
    }

    /// §20.2 relative-path resolution, exercised through `resolve_module`
    /// directly: implicit extension, then `index.*` directory resolution.
    #[test]
    fn relative_import_resolves_implicit_extension_and_index() {
        let known: HashSet<String> = ["src/app.ts", "src/utils.ts", "src/ui/index.ts"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let aliases = TsconfigAliases::new();

        let utils = resolve_module("src/app.ts", "./utils", &known, &aliases);
        assert!(matches!(utils, ModuleResolution::Project(p) if p == "src/utils.ts"));

        let ui = resolve_module("src/app.ts", "./ui", &known, &aliases);
        assert!(matches!(ui, ModuleResolution::Project(p) if p == "src/ui/index.ts"));
    }

    /// A relative specifier that matches no project file (spec
    /// scenario-adjacent to "Zero-candidate import").
    #[test]
    fn relative_import_to_missing_file_is_unresolved() {
        let known: HashSet<String> = ["src/app.ts".to_string()].into_iter().collect();
        let result = resolve_module(
            "src/app.ts",
            "./nonexistent",
            &known,
            &TsconfigAliases::new(),
        );
        assert!(matches!(result, ModuleResolution::Unresolved));
    }

    /// A single-`*`-prefix tsconfig alias resolves like a relative import.
    #[test]
    fn tsconfig_alias_resolves_to_project_file() {
        let known: HashSet<String> = ["src/main.ts", "src/utils.ts"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let mut aliases = TsconfigAliases::new();
        aliases.insert("@app/*".to_string(), "src/*".to_string());

        let result = resolve_module("src/main.ts", "@app/utils", &known, &aliases);
        assert!(matches!(result, ModuleResolution::Project(p) if p == "src/utils.ts"));
    }

    /// A bare package specifier resolves to an external node (spec
    /// "External package import"), never a project/unresolved outcome.
    #[test]
    fn bare_specifier_resolves_external() {
        let known: HashSet<String> = ["src/main.ts".to_string()].into_iter().collect();
        let result = resolve_module("src/main.ts", "zod", &known, &TsconfigAliases::new());
        assert!(matches!(result, ModuleResolution::External(pkg) if pkg == "zod"));
    }

    /// `SymbolTable::build`'s direct-declaration-export fallback: a plain
    /// `export function x() {}` (no `Exports` edge from extract.rs) is
    /// still importable by name, and registered in `by_name` too (project-
    /// wide by-name lookup, PR4a-2).
    #[test]
    fn symbol_table_registers_direct_declaration_exports() {
        let files = vec![analyzed("src/utils.ts", "export function helper() {}\n")];
        let table = SymbolTable::build(&files);

        let export = table
            .exports
            .get("src/utils.ts")
            .and_then(|m| m.get("helper"))
            .unwrap();
        assert_eq!(export.file, "src/utils.ts");
        assert_eq!(export.qualified_name, "src/utils.ts::helper");
        assert_eq!(export.kind, SymbolKind::Function);

        let by_name = table.by_name.get("helper").unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].qualified_name, "src/utils.ts::helper");
    }
}
