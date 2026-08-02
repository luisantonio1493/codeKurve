//! Whole-project resolution (design §Interfaces, plan §20.2/§20.4): turns
//! the by-name/module-specifier `EdgeTarget::Unresolved` edges every
//! `extract::analyze` call leaves behind into `EdgeTarget::Global`/`External`
//! edges (or `UnresolvedReference` rows, never silently dropped — §18.3),
//! once every file in the run has been parsed. Pure library, no filesystem
//! I/O: it only ever looks at the `FileAnalysis.file` paths already produced
//! by discovery + `extract::analyze`, so no project-root parameter is
//! needed here (PR4b's pipeline is what makes that file set complete).
//!
//! PR4a-1 shipped the whole-project symbol table and module resolution
//! (§20.2, still below); this adds reference/call resolution against that
//! table (§20.4): `resolve()` walks every `Unresolved` edge and either
//! resolves it to `Global`/`External`, or moves it to
//! `FileAnalysis.unresolved` when it has zero candidates.

use std::collections::{HashMap, HashSet};

use codekurve_core::{Confidence, LanguageId, Provenance, RelationshipKind, SymbolKind};

use crate::ir::{EdgeTarget, ExtractedRelationship, FileAnalysis, UnresolvedReference};
use crate::languages::{analyzer_for, kind_matches, same_resolution_domain, BASE_LIST_REASON};

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
    pub(crate) language: LanguageId,
}

/// One symbol read back from prior storage — everything `SymbolTable::build`
/// needs to seed `by_name`/`exports` without re-parsing the file it came
/// from (design "Baseline for re-resolution", Phase 3 task 4.2). `exported`
/// mirrors what a fresh parse's `SymbolTable::build` fallback records: every
/// top-level Class/Function/Interface counts as its own module's export
/// (see the fallback loop below) — the store side
/// (`codekurve-store::repo::resolution_snapshot`) derives the same signal
/// from a persisted `kind = 'exports'` relationship targeting the symbol.
#[derive(Debug, Clone)]
pub struct BaselineSymbol {
    pub name: String,
    pub file: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub language: LanguageId,
    pub exported: bool,
}

/// Prior-index snapshot fed into `resolve_with` for an incremental batch
/// (design "Baseline for re-resolution"): symbols and known file paths from
/// files this batch is NOT re-parsing, so cross-file resolution against
/// already-indexed files works without re-parsing them. `EMPTY` is what
/// `resolve()`'s full-reindex path seeds with — every file is freshly
/// parsed there, so there is nothing to seed.
#[derive(Debug, Clone, Default)]
pub struct ProjectBaseline {
    files: Vec<String>,
    symbols: Vec<BaselineSymbol>,
}

impl ProjectBaseline {
    pub const EMPTY: ProjectBaseline = ProjectBaseline {
        files: Vec::new(),
        symbols: Vec::new(),
    };

    /// Composition-root constructor (task 4.4): `codekurve/src/commands.rs`
    /// maps `codekurve-store::repo::resolution_snapshot`'s rows into this —
    /// `codekurve-store` never depends on `codekurve-analysis`, so the glue
    /// lives in the binary crate instead.
    pub fn new(files: Vec<String>, symbols: Vec<BaselineSymbol>) -> Self {
        Self { files, symbols }
    }
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
        Self::build_with(files, &ProjectBaseline::EMPTY)
    }

    /// Task 4.2 (design "Baseline for re-resolution"): seeds `by_name`/
    /// `exports` from `baseline` BEFORE folding in `files`' fresh per-file
    /// analyses below, so a batch that only reparsed the affected set still
    /// resolves against every already-indexed symbol without re-parsing it.
    pub(crate) fn build_with(files: &[FileAnalysis], baseline: &ProjectBaseline) -> Self {
        let mut by_name: HashMap<String, Vec<ProjectSymbol>> = HashMap::new();
        let mut exports: HashMap<String, HashMap<String, ProjectSymbol>> = HashMap::new();

        for entry in &baseline.symbols {
            let ps = ProjectSymbol {
                file: entry.file.clone(),
                qualified_name: entry.qualified_name.clone(),
                kind: entry.kind,
                language: entry.language,
            };
            by_name
                .entry(entry.name.clone())
                .or_default()
                .push(ps.clone());
            if entry.exported {
                exports
                    .entry(entry.file.clone())
                    .or_default()
                    .insert(entry.name.clone(), ps);
            }
        }

        for file in files {
            for sym in &file.symbols {
                by_name
                    .entry(sym.name.clone())
                    .or_default()
                    .push(ProjectSymbol {
                        file: file.file.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind,
                        language: file.language,
                    });
            }
        }

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
                        language: file.language,
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
                            language: file.language,
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

/// Counts of how many edges resolution produced, for `codekurve index`'s
/// summary output (PR4b).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionReport {
    pub resolved: usize,
    pub unresolved: usize,
}

/// Resolves every `EdgeTarget::Unresolved` edge left by `extract::analyze`
/// against the whole-project `SymbolTable`, in place. Zero-candidate edges
/// move to `FileAnalysis.unresolved` (never dropped, §18.3); multi-candidate
/// edges become one Low-confidence edge per candidate (never pick first,
/// §20.4). Thin wrapper (task 4.1) — a full reindex has already parsed
/// every file, so there is no baseline to seed `resolve_with` from.
pub fn resolve(files: &mut [FileAnalysis], aliases: &TsconfigAliases) -> ResolutionReport {
    resolve_with(files, aliases, &ProjectBaseline::EMPTY)
}

/// Task 4.1 (design "Baseline for re-resolution"): same as `resolve`, but
/// seeds the whole-project `SymbolTable` from `baseline` first — an
/// incremental batch's affected-set files can then resolve cross-file edges
/// (both by-name and module-specifier imports) against already-indexed
/// files without re-parsing them.
pub fn resolve_with(
    files: &mut [FileAnalysis],
    aliases: &TsconfigAliases,
    baseline: &ProjectBaseline,
) -> ResolutionReport {
    let table = SymbolTable::build_with(&*files, baseline);
    let mut known_files: HashSet<String> = files.iter().map(|f| f.file.clone()).collect();
    known_files.extend(baseline.files.iter().cloned());
    let mut report = ResolutionReport::default();

    for file in files.iter_mut() {
        let file_name = file.file.clone();
        let source_language = file.language;
        let old_rels = std::mem::take(&mut file.relationships);
        let mut new_rels = Vec::with_capacity(old_rels.len());
        for rel in old_rels {
            resolve_one(
                &file_name,
                source_language,
                rel,
                &table,
                &known_files,
                aliases,
                &mut new_rels,
                &mut file.unresolved,
                &mut report,
            );
        }
        file.relationships = new_rels;
    }
    report
}

#[allow(clippy::too_many_arguments)]
fn resolve_one(
    file: &str,
    source_language: LanguageId,
    rel: ExtractedRelationship,
    table: &SymbolTable,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    let text = match &rel.target {
        EdgeTarget::Unresolved(t) => t.clone(),
        _ => {
            new_rels.push(rel);
            return;
        }
    };

    match rel.kind {
        RelationshipKind::Imports if source_language == LanguageId::CSharp => {
            resolve_using(&rel, &text, table, new_rels, report)
        }
        RelationshipKind::Imports => resolve_import(
            file,
            &rel,
            &text,
            table,
            known_files,
            aliases,
            new_rels,
            unresolved,
            report,
        ),
        RelationshipKind::Exports => resolve_export(
            file,
            &rel,
            &text,
            table,
            known_files,
            aliases,
            new_rels,
            unresolved,
            report,
        ),
        RelationshipKind::UsesType if rel.reason.as_deref() == Some(BASE_LIST_REASON) => {
            resolve_base_entry(
                source_language,
                &rel,
                &text,
                table,
                new_rels,
                unresolved,
                report,
            )
        }
        RelationshipKind::Calls
        | RelationshipKind::Constructs
        | RelationshipKind::Inherits
        | RelationshipKind::Implements
        | RelationshipKind::References => resolve_by_name(
            source_language,
            &rel,
            &text,
            table,
            new_rels,
            unresolved,
            report,
        ),
        // D4/D5: the 5 framework-level kinds always arrive as
        // `EdgeTarget::Unresolved(<name as written>)` from `frameworks::
        // recognize`, bound here through the same by-name project lookup
        // every other kind uses — but never promoted past the D5 provenance
        // floor (`resolve_framework_edge`/`push_framework_edge` below).
        RelationshipKind::Injects
        | RelationshipKind::RegisteredAs
        | RelationshipKind::HandlesRoute
        | RelationshipKind::Triggers
        | RelationshipKind::PersistsTo => resolve_framework_edge(
            source_language,
            &rel,
            &text,
            table,
            new_rels,
            unresolved,
            report,
        ),
        // `Defines`/`Overrides`/`UsesType`/`Reads`/`Writes` aren't produced
        // by `extract::analyze` yet — nothing to resolve, pass through
        // unchanged.
        _ => new_rels.push(rel),
    }
}

/// §20.4 tiers for a project-wide by-name lookup (`Calls`/`Constructs`
/// same-file misses, `Inherits`/`Implements` same-file misses).
/// `kind_matches` dispatches through the reference's own source-file
/// analyzer (design "Resolution Changes") rather than a shared free
/// function, so a second language never silently inherits TypeScript's
/// rules; language-filtered candidates land in a later PR (PR5).
fn resolve_by_name(
    source_language: LanguageId,
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    let analyzer = analyzer_for(source_language);
    let matches: Vec<&ProjectSymbol> = table
        .by_name
        .get(text)
        .into_iter()
        .flatten()
        .filter(|ps| {
            same_resolution_domain(source_language, ps.language)
                && kind_matches(analyzer, rel.kind, ps.kind)
        })
        .collect();

    match matches.as_slice() {
        [] => {
            unresolved.push(unresolved_ref(rel, text, "no matching symbol in project"));
            report.unresolved += 1;
        }
        [only] => {
            push_global(
                new_rels,
                rel,
                &only.file,
                &only.qualified_name,
                Provenance::Resolved,
                Confidence::High,
            );
            report.resolved += 1;
        }
        many => {
            // Never silently pick one (§20.4/§27.4): one Low-confidence,
            // Heuristic-provenance edge per candidate — extraction collapses
            // both `this.foo()`/`obj.foo()` member calls and bare `foo()`
            // calls to the same by-name target, so a multi-candidate hit is
            // always a genuine "receiver undeterminable" case here.
            for candidate in many {
                push_global(
                    new_rels,
                    rel,
                    &candidate.file,
                    &candidate.qualified_name,
                    Provenance::Heuristic,
                    Confidence::Low,
                );
                report.resolved += 1;
            }
        }
    }
}

/// D5 provenance floor's confidence side: `Exact > High > Medium > Low >
/// Unresolved`, as a rank where a *larger* number is weaker. `min_confidence`
/// picks whichever of the two inputs is weaker — the recognition-time
/// ceiling never gets upgraded by a clean resolution outcome.
fn confidence_rank(c: Confidence) -> u8 {
    match c {
        Confidence::Exact => 0,
        Confidence::High => 1,
        Confidence::Medium => 2,
        Confidence::Low => 3,
        Confidence::Unresolved => 4,
    }
}

fn min_confidence(a: Confidence, b: Confidence) -> Confidence {
    if confidence_rank(a) >= confidence_rank(b) {
        a
    } else {
        b
    }
}

/// Project-wide by-name lookup for the 5 framework-level kinds (D4), sharing
/// `resolve_by_name`'s candidate search but never `resolve_by_name`'s
/// `push_global` — a framework edge is bound through `push_framework_edge`
/// instead, which is what enforces the D5 provenance floor.
fn resolve_framework_edge(
    source_language: LanguageId,
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    let analyzer = analyzer_for(source_language);
    let matches: Vec<&ProjectSymbol> = table
        .by_name
        .get(text)
        .into_iter()
        .flatten()
        .filter(|ps| {
            same_resolution_domain(source_language, ps.language)
                && kind_matches(analyzer, rel.kind, ps.kind)
        })
        .collect();

    match matches.as_slice() {
        [] => {
            unresolved.push(unresolved_ref(rel, text, "no matching symbol in project"));
            report.unresolved += 1;
        }
        [only] => {
            // Q1 resolution table's interface cap (design.md "Q1 — Angular
            // and .NET DI inference"), scoped to `Injects` only: a single
            // `Interface` candidate never resolves past `Medium` — an
            // interface names a contract, not the implementation that runs,
            // and only a future `RegisteredAs` edge says which impl that is.
            // Every other framework kind (and every non-Interface `Injects`
            // candidate) keeps the pre-existing single-candidate `High`.
            let resolution_confidence =
                if rel.kind == RelationshipKind::Injects && only.kind == SymbolKind::Interface {
                    Confidence::Medium
                } else {
                    Confidence::High
                };
            push_framework_edge(
                new_rels,
                rel,
                &only.file,
                &only.qualified_name,
                resolution_confidence,
            );
            report.resolved += 1;
        }
        many => {
            for candidate in many {
                push_framework_edge(
                    new_rels,
                    rel,
                    &candidate.file,
                    &candidate.qualified_name,
                    Confidence::Low,
                );
                report.resolved += 1;
            }
        }
    }
}

/// D5 provenance floor, load-bearing part of this PR: `provenance` is
/// carried through from `rel` verbatim — a `Heuristic` edge stays
/// `Heuristic`, *never* upgraded to `Resolved`/`Extracted` no matter how
/// clean the resolution match was. `confidence` is capped at `min(rel's
/// recognition-time ceiling, this resolution's own confidence)`, so a
/// single-candidate match can only ever *keep or lower* the ceiling the
/// recognition pass assigned, never raise it. This is what keeps a
/// heuristic guess from ever becoming indistinguishable from a parsed fact.
fn push_framework_edge(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    file: &str,
    qualified_name: &str,
    resolution_confidence: Confidence,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::Global {
            file: file.to_string(),
            qualified_name: qualified_name.to_string(),
        },
        kind: rel.kind,
        span: rel.span,
        provenance: rel.provenance,
        confidence: min_confidence(rel.confidence, resolution_confidence),
        reason: rel.reason.clone(),
    });
}

fn resolve_base_entry(
    source_language: LanguageId,
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    let matches: Vec<&ProjectSymbol> = table
        .by_name
        .get(text)
        .into_iter()
        .flatten()
        .filter(|ps| {
            same_resolution_domain(source_language, ps.language)
                && matches!(
                    ps.kind,
                    SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface
                )
        })
        .collect();

    match matches.as_slice() {
        [] => {
            unresolved.push(unresolved_ref(
                rel,
                text,
                "base list entry not found in project; class vs interface undeterminable",
            ));
            report.unresolved += 1;
        }
        [only] => {
            push_classified_base(new_rels, rel, only, Provenance::Resolved, Confidence::High);
            report.resolved += 1;
        }
        many => {
            for candidate in many {
                push_classified_base(
                    new_rels,
                    rel,
                    candidate,
                    Provenance::Heuristic,
                    Confidence::Low,
                );
                report.resolved += 1;
            }
        }
    }
}

fn push_classified_base(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    candidate: &ProjectSymbol,
    provenance: Provenance,
    confidence: Confidence,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::Global {
            file: candidate.file.clone(),
            qualified_name: candidate.qualified_name.clone(),
        },
        kind: if candidate.kind == SymbolKind::Interface {
            RelationshipKind::Implements
        } else {
            RelationshipKind::Inherits
        },
        span: rel.span,
        provenance,
        confidence,
        reason: None,
    });
}

fn resolve_using(
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    report: &mut ResolutionReport,
) {
    let matches: Vec<&ProjectSymbol> = table
        .by_name
        .get(text)
        .into_iter()
        .flatten()
        .filter(|ps| ps.language == LanguageId::CSharp && ps.kind == SymbolKind::Namespace)
        .collect();

    match matches.as_slice() {
        [] => {
            push_external(new_rels, rel, text.to_string());
            report.resolved += 1;
        }
        [only] => {
            push_global_preserving_reason(
                new_rels,
                rel,
                only,
                Provenance::Resolved,
                Confidence::High,
            );
            report.resolved += 1;
        }
        many => {
            for candidate in many {
                push_global_preserving_reason(
                    new_rels,
                    rel,
                    candidate,
                    Provenance::Heuristic,
                    Confidence::Low,
                );
                report.resolved += 1;
            }
        }
    }
}

fn push_global_preserving_reason(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    candidate: &ProjectSymbol,
    provenance: Provenance,
    confidence: Confidence,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::Global {
            file: candidate.file.clone(),
            qualified_name: candidate.qualified_name.clone(),
        },
        kind: rel.kind,
        span: rel.span,
        provenance,
        confidence,
        reason: rel.reason.clone(),
    });
}

/// An `Imports` edge, or an `Exports` edge with a `from` module specifier.
#[allow(clippy::too_many_arguments)]
fn resolve_import(
    file: &str,
    rel: &ExtractedRelationship,
    specifier: &str,
    table: &SymbolTable,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    match resolve_module(file, specifier, known_files, aliases) {
        ModuleResolution::External(pkg) => {
            push_external(new_rels, rel, pkg);
            report.resolved += 1;
        }
        ModuleResolution::Unresolved => {
            unresolved.push(unresolved_ref(
                rel,
                specifier,
                "module not found in project",
            ));
            report.unresolved += 1;
        }
        ModuleResolution::Project(target_file) => resolve_binding(
            rel,
            &target_file,
            rel.reason.as_deref(),
            table,
            new_rels,
            unresolved,
            report,
        ),
    }
}

/// An `Exports` edge: either a same-file export with zero local candidates
/// (extract.rs already searched the file — can never resolve elsewhere), or
/// a module-specifier re-export (`export { x } from './mod'` / `export *
/// from './mod'`).
#[allow(clippy::too_many_arguments)]
fn resolve_export(
    file: &str,
    rel: &ExtractedRelationship,
    text: &str,
    table: &SymbolTable,
    known_files: &HashSet<String>,
    aliases: &TsconfigAliases,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    if rel.reason.as_deref() == Some(crate::extract::NO_SAME_FILE_MATCH_REASON) {
        unresolved.push(unresolved_ref(rel, text, "not declared in this file"));
        report.unresolved += 1;
        return;
    }

    match resolve_module(file, text, known_files, aliases) {
        ModuleResolution::External(pkg) => {
            push_external(new_rels, rel, pkg);
            report.resolved += 1;
        }
        ModuleResolution::Unresolved => {
            unresolved.push(unresolved_ref(rel, text, "module not found in project"));
            report.unresolved += 1;
        }
        ModuleResolution::Project(target_file) => resolve_binding(
            rel,
            &target_file,
            rel.reason.as_deref(),
            table,
            new_rels,
            unresolved,
            report,
        ),
    }
}

/// Resolves a named binding (`Some(name)`), namespace import/blanket
/// re-export (`Some("*")`/`None`) against a module already resolved to a
/// project file.
fn resolve_binding(
    rel: &ExtractedRelationship,
    target_file: &str,
    binding: Option<&str>,
    table: &SymbolTable,
    new_rels: &mut Vec<ExtractedRelationship>,
    unresolved: &mut Vec<UnresolvedReference>,
    report: &mut ResolutionReport,
) {
    match binding {
        Some("*") | None => {
            // Namespace import / `export * from`: binds the whole module,
            // not one symbol — no `Module` symbol kind is emitted yet
            // (ponytail: add one if per-namespace-member access is needed).
            push_global(
                new_rels,
                rel,
                target_file,
                "*",
                Provenance::Resolved,
                Confidence::High,
            );
            report.resolved += 1;
        }
        Some(name) => match table.exports.get(target_file).and_then(|m| m.get(name)) {
            Some(sym) => {
                push_global(
                    new_rels,
                    rel,
                    &sym.file,
                    &sym.qualified_name,
                    Provenance::Resolved,
                    Confidence::Exact,
                );
                report.resolved += 1;
            }
            None => {
                // A `"default"` reason also lands here: extraction doesn't
                // tag which local export is the module's `export default`
                // target (see extract.rs's `collect_exports` doc comment),
                // so a literal `"default"` key is never present — reported
                // as unresolved rather than guessed.
                unresolved.push(unresolved_ref(rel, name, "not exported by target module"));
                report.unresolved += 1;
            }
        },
    }
}

fn push_global(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    file: &str,
    qualified_name: &str,
    provenance: Provenance,
    confidence: Confidence,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::Global {
            file: file.to_string(),
            qualified_name: qualified_name.to_string(),
        },
        kind: rel.kind,
        span: rel.span,
        provenance,
        confidence,
        reason: None,
    });
}

fn push_external(
    new_rels: &mut Vec<ExtractedRelationship>,
    rel: &ExtractedRelationship,
    pkg: String,
) {
    new_rels.push(ExtractedRelationship {
        source_local_key: rel.source_local_key.clone(),
        target: EdgeTarget::External(pkg),
        kind: rel.kind,
        span: rel.span,
        provenance: Provenance::Resolved,
        confidence: Confidence::Exact,
        reason: rel.reason.clone(),
    });
}

fn unresolved_ref(
    rel: &ExtractedRelationship,
    target_text: &str,
    reason: &str,
) -> UnresolvedReference {
    UnresolvedReference {
        source_local_key: rel.source_local_key.clone(),
        relationship_kind: rel.kind,
        target_text: target_text.to_string(),
        context: rel.reason.clone(),
        candidate_count: 0,
        reason: reason.to_string(),
        confidence: Confidence::Unresolved,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::analyze;
    use codekurve_core::{LanguageId, SourceSpan};

    fn analyzed(path: &str, source: &str) -> FileAnalysis {
        analyze(source, LanguageId::TypeScript, path).unwrap()
    }

    fn csharp(path: &str, source: &str) -> FileAnalysis {
        analyze(source, LanguageId::CSharp, path).unwrap()
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

    /// Relative import + implicit extension/`index.*`, end to end through
    /// `resolve()` (spec "Implicit extension resolution").
    #[test]
    fn resolve_named_import_across_files() {
        let mut files = vec![
            analyzed("src/app.ts", "import { helper } from './utils';\n"),
            analyzed("src/utils.ts", "export function helper() {}\n"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);

        let edge = files[0].relationships.first().unwrap();
        assert_eq!(
            edge.target,
            EdgeTarget::Global {
                file: "src/utils.ts".to_string(),
                qualified_name: "src/utils.ts::helper".to_string(),
            }
        );
        assert_eq!(edge.confidence, Confidence::Exact);
    }

    /// A bare package specifier resolves to an external node, never an
    /// `UnresolvedReference` row (spec "External package import").
    #[test]
    fn external_package_import_has_no_unresolved_row() {
        let mut files = vec![analyzed("src/main.ts", "import { z } from 'zod';\n")];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);
        assert!(files[0].unresolved.is_empty());
        assert_eq!(
            files[0].relationships.first().unwrap().target,
            EdgeTarget::External("zod".to_string())
        );
    }

    /// A same-file-unresolved `Calls` edge resolves to the single cross-file
    /// candidate at High confidence (spec-adjacent to "Exact local call").
    #[test]
    fn cross_file_call_resolves_to_single_candidate() {
        let mut files = vec![
            analyzed("src/caller.ts", "function run() { return doWork(); }\n"),
            analyzed("src/worker.ts", "export function doWork() { return 1; }\n"),
        ];

        resolve(&mut files, &TsconfigAliases::new());

        let call = files[0]
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Calls)
            .unwrap();
        assert_eq!(
            call.target,
            EdgeTarget::Global {
                file: "src/worker.ts".to_string(),
                qualified_name: "src/worker.ts::doWork".to_string(),
            }
        );
        assert_eq!(call.provenance, Provenance::Resolved);
        assert_eq!(call.confidence, Confidence::High);
    }

    /// Two cross-file candidates -> two Low-confidence edges, never pick
    /// one (spec "Multi-candidate call is not unresolved").
    #[test]
    fn multi_candidate_call_produces_one_low_confidence_edge_per_candidate() {
        let mut files = vec![
            analyzed("src/caller.ts", "function run() { return doWork(); }\n"),
            analyzed("src/worker1.ts", "export function doWork() { return 1; }\n"),
            analyzed("src/worker2.ts", "export function doWork() { return 2; }\n"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);

        let calls: Vec<_> = files[0]
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .collect();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|r| r.confidence == Confidence::Low));
        assert!(calls.iter().all(|r| r.provenance == Provenance::Heuristic));
        assert!(files[0].unresolved.is_empty());
    }

    /// Task 4.1/4.2 (design "Baseline for re-resolution"): `resolve_with`
    /// resolves a batch file's call/import against a `ProjectBaseline`
    /// symbol from a file that is NOT in `files` at all (never re-parsed),
    /// exercising both `by_name` (`Calls`) and `exports`/`known_files`
    /// (`Imports`) seeding in one pass.
    #[test]
    fn resolve_with_baseline_resolves_against_unparsed_file() {
        let mut files = vec![analyzed(
            "src/caller.ts",
            "import { doWork } from './worker';\nfunction run() { return doWork(); }\n",
        )];
        let baseline = ProjectBaseline::new(
            vec!["src/worker.ts".to_string()],
            vec![BaselineSymbol {
                name: "doWork".to_string(),
                file: "src/worker.ts".to_string(),
                qualified_name: "src/worker.ts::doWork".to_string(),
                kind: SymbolKind::Function,
                language: LanguageId::TypeScript,
                exported: true,
            }],
        );

        let report = resolve_with(&mut files, &TsconfigAliases::new(), &baseline);
        assert_eq!(report.unresolved, 0);

        let expected = EdgeTarget::Global {
            file: "src/worker.ts".to_string(),
            qualified_name: "src/worker.ts::doWork".to_string(),
        };
        let import = files[0]
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Imports)
            .unwrap();
        assert_eq!(import.target, expected);
        let call = files[0]
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Calls)
            .unwrap();
        assert_eq!(call.target, expected);
    }

    /// `resolve()` still delegates to `EMPTY` (zero regression, task 4.1):
    /// with no baseline, the same unparsed-file import is legitimately
    /// unresolved.
    #[test]
    fn resolve_without_baseline_leaves_unparsed_file_unresolved() {
        let mut files = vec![analyzed(
            "src/caller.ts",
            "import { doWork } from './worker';\n",
        )];
        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 1);
    }

    /// Zero candidates anywhere in the project -> `UnresolvedReference`,
    /// never dropped and never left as a relationships-table row (spec
    /// "Zero-candidate import" principle applies equally to calls).
    #[test]
    fn zero_candidate_call_becomes_unresolved_reference() {
        let mut files = vec![analyzed(
            "src/lonely.ts",
            "function run() { return neverDefined(); }\n",
        )];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 1);

        assert!(!files[0]
            .relationships
            .iter()
            .any(|r| r.kind == RelationshipKind::Calls));

        let unresolved = files[0].unresolved.first().unwrap();
        assert_eq!(unresolved.relationship_kind, RelationshipKind::Calls);
        assert_eq!(unresolved.target_text, "neverDefined");
        assert_eq!(unresolved.candidate_count, 0);
        assert_eq!(unresolved.confidence, Confidence::Unresolved);
    }

    #[test]
    fn csharp_base_list_resolves_classes_and_interfaces_across_files() {
        let mut files = vec![
            csharp(
                "src/base.cs",
                "namespace Acme { public class Base {} public interface IBillable {} }",
            ),
            csharp(
                "src/invoice.cs",
                "namespace Acme { public class Invoice : Base, IBillable {} }",
            ),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);
        let edges = &files[1].relationships;
        assert!(edges
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Inherits
                && edge.target
                    == EdgeTarget::Global {
                        file: "src/base.cs".to_string(),
                        qualified_name: "src/base.cs::Acme.Base".to_string()
                    }
                && edge.confidence == Confidence::High));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Implements
                && edge.target
                    == EdgeTarget::Global {
                        file: "src/base.cs".to_string(),
                        qualified_name: "src/base.cs::Acme.IBillable".to_string()
                    }
                && edge.confidence == Confidence::High));
    }

    #[test]
    fn csharp_unresolved_base_list_is_preserved_without_a_guess() {
        let mut files = vec![csharp(
            "src/invoice.cs",
            "namespace Acme { public class Invoice : MissingBase {} }",
        )];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 1);
        assert!(files[0]
            .relationships
            .iter()
            .all(|edge| edge.kind != RelationshipKind::UsesType));
        assert!(files[0].unresolved.iter().any(|reference| {
            reference.relationship_kind == RelationshipKind::UsesType
                && reference.target_text == "MissingBase"
                && reference.reason
                    == "base list entry not found in project; class vs interface undeterminable"
        }));
    }

    #[test]
    fn language_filter_prevents_cross_language_resolution() {
        let mut files = vec![
            analyzed("src/caller.ts", "function run() { return new Invoice(); }"),
            csharp("src/invoice.cs", "public class Invoice {}"),
            csharp(
                "src/caller.cs",
                "public class Caller { void Run() { new TsOnly(); } }",
            ),
            analyzed("src/ts-only.ts", "export class TsOnly {}"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 2);
        assert!(files[0]
            .relationships
            .iter()
            .all(|edge| edge.kind != RelationshipKind::Constructs));
        assert!(files[2]
            .relationships
            .iter()
            .all(|edge| edge.kind != RelationshipKind::Constructs));
    }

    #[test]
    fn csharp_visibility_does_not_change_unambiguous_call_confidence() {
        let mut files = vec![
            csharp("src/targets.cs", "public class Targets { public void PublicTarget() {} internal void InternalTarget() {} protected internal void ProtectedInternalTarget() {} private protected void PrivateProtectedTarget() {} }"),
            csharp("src/caller.cs", "public class Caller { void Run() { PublicTarget(); InternalTarget(); ProtectedInternalTarget(); PrivateProtectedTarget(); } }"),
        ];

        resolve(&mut files, &TsconfigAliases::new());
        let calls: Vec<_> = files[1]
            .relationships
            .iter()
            .filter(|edge| edge.kind == RelationshipKind::Calls)
            .collect();
        assert_eq!(calls.len(), 4);
        assert!(calls.iter().all(|edge| edge.confidence == Confidence::High));
    }

    #[test]
    fn csharp_partial_type_reference_keeps_each_fragment_as_a_low_confidence_candidate() {
        let mut files = vec![
            csharp("src/first.cs", "public partial class Invoice {}"),
            csharp("src/second.cs", "public partial class Invoice {}"),
            csharp(
                "src/caller.cs",
                "public class Caller { void Run() { new Invoice(); } }",
            ),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);
        let constructs: Vec<_> = files[2]
            .relationships
            .iter()
            .filter(|edge| edge.kind == RelationshipKind::Constructs)
            .collect();
        assert_eq!(constructs.len(), 2);
        assert!(constructs
            .iter()
            .all(|edge| edge.confidence == Confidence::Low));
    }

    #[test]
    fn typescript_cross_file_calls_still_resolve_in_any_parse_order() {
        let mut files = vec![
            analyzed("src/worker.ts", "export function doWork() { return 1; }"),
            analyzed("src/caller.ts", "function run() { return doWork(); }"),
        ];

        resolve(&mut files, &TsconfigAliases::new());
        assert!(files[1]
            .relationships
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Calls
                && edge.target
                    == EdgeTarget::Global {
                        file: "src/worker.ts".to_string(),
                        qualified_name: "src/worker.ts::doWork".to_string()
                    }));
    }

    #[test]
    fn csharp_multifile_runtime_resolution_preserves_unresolved_rows() {
        let mut files = vec![
            csharp("src/Billing.cs", "namespace Acme.Billing { public class BillingDocument {} public interface IBillable {} }"),
            csharp("src/Worker.cs", "namespace Acme.App { public class Worker { public void Execute() {} } }"),
            csharp("src/Invoice.cs", "using Acme.Billing; namespace Acme.App { public class Invoice : BillingDocument, IBillable { void Run() { new Worker(); Execute(); new Missing(); } } }"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 1);
        let edges = &files[2].relationships;
        assert!(edges
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Imports
                && matches!(edge.target, EdgeTarget::Global { .. })));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Inherits
                && matches!(edge.target, EdgeTarget::Global { .. })));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Implements
                && matches!(edge.target, EdgeTarget::Global { .. })));
        assert!(edges
            .iter()
            .any(|edge| edge.kind == RelationshipKind::Constructs
                && matches!(edge.target, EdgeTarget::Global { .. })));
        assert!(edges.iter().any(|edge| edge.kind == RelationshipKind::Calls
            && matches!(edge.target, EdgeTarget::Global { .. })));
        assert!(files[2]
            .unresolved
            .iter()
            .any(|reference| reference.target_text == "Missing"));
    }

    /// `using static` only affects the `Imports` edge (task 4.1/design
    /// "Resolution Changes") — it does not make an unqualified call to a
    /// statically-imported member resolvable. `WriteLine` has no project
    /// symbol, so the `Calls` edge must land in `unresolved` with the
    /// generic by-name reason, never silently dropped or guessed.
    #[test]
    fn using_static_call_site_stays_unresolved() {
        let mut files = vec![csharp(
            "src/program.cs",
            "using static System.Console; public class Program { void Run() { WriteLine(\"hi\"); } }",
        )];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert!(files[0]
            .relationships
            .iter()
            .all(|edge| edge.kind != RelationshipKind::Calls));
        assert!(files[0].unresolved.iter().any(|reference| {
            reference.relationship_kind == RelationshipKind::Calls
                && reference.target_text == "WriteLine"
                && reference.reason == "no matching symbol in project"
        }));
        assert!(report.unresolved >= 1);
    }

    /// `using Alias = X.Y;` introduces no project symbol named `Alias`
    /// (design "Resolution Changes" — aliases are never expanded at
    /// resolve time). Constructing `new Alias()` must therefore land in
    /// `unresolved` with an explicit reason, never guessed or dropped.
    #[test]
    fn alias_qualified_reference_stays_unresolved_with_reason() {
        let mut files = vec![csharp(
            "src/program.cs",
            "using Alias = System.Collections.Generic.List<int>; public class Program { void Run() { new Alias(); } }",
        )];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert!(files[0]
            .relationships
            .iter()
            .all(|edge| edge.kind != RelationshipKind::Constructs));
        assert!(files[0].unresolved.iter().any(|reference| {
            reference.relationship_kind == RelationshipKind::Constructs
                && reference.target_text == "Alias"
                && reference.reason == "no matching symbol in project"
        }));
        assert!(report.unresolved >= 1);
    }

    /// Task 3.9 — the D5 provenance floor, unit-tested with a synthetic
    /// edge *before any catalogue exists* (PR4/5/6 land later): a
    /// `Heuristic`/`Medium` framework edge with exactly one project-wide
    /// candidate must resolve to `Heuristic`/`Medium`, never `Resolved`/
    /// `Exact` — proving the floor holds even though the by-name lookup
    /// below is the exact same "exactly one candidate" shape that gives
    /// `resolve_by_name` `Provenance::Resolved`/`Confidence::High`.
    #[test]
    fn provenance_floor_never_upgrades_a_heuristic_framework_edge() {
        let dummy_span = SourceSpan {
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            start_column: 0,
            end_line: 1,
            end_column: 0,
        };
        let synthetic = FileAnalysis {
            file: "src/widget.ts".to_string(),
            language: LanguageId::TypeScript,
            symbols: vec![],
            relationships: vec![ExtractedRelationship {
                source_local_key: "widget".to_string(),
                target: EdgeTarget::Unresolved("Service".to_string()),
                kind: RelationshipKind::Injects,
                span: dummy_span,
                provenance: Provenance::Heuristic,
                confidence: Confidence::Medium,
                reason: Some("di:ctor-param:0".to_string()),
            }],
            unresolved: vec![],
            diagnostics: vec![],
        };
        let mut files = vec![
            synthetic,
            analyzed("src/service.ts", "export class Service {}"),
        ];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 0);

        let edge = files[0]
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Injects)
            .expect("Injects edge must survive resolution");

        // Exactly one candidate ("Service" the class) would give
        // `resolve_by_name` `Provenance::Resolved`/`Confidence::High` — the
        // floor must keep provenance at `Heuristic` and cap confidence at
        // the recognition-time ceiling (`Medium`), never let resolution
        // raise it to `High`.
        assert_eq!(edge.provenance, Provenance::Heuristic);
        assert_eq!(edge.confidence, Confidence::Medium);
        assert_eq!(
            edge.target,
            EdgeTarget::Global {
                file: "src/service.ts".to_string(),
                qualified_name: "src/service.ts::Service".to_string(),
            }
        );
    }

    /// Same shape, zero candidates: an `UnresolvedReference` is preserved
    /// exactly as any other kind's zero-candidate case, never dropped.
    #[test]
    fn provenance_floor_zero_candidates_becomes_unresolved_reference() {
        let dummy_span = SourceSpan {
            start_byte: 0,
            end_byte: 0,
            start_line: 1,
            start_column: 0,
            end_line: 1,
            end_column: 0,
        };
        let synthetic = FileAnalysis {
            file: "src/widget.ts".to_string(),
            language: LanguageId::TypeScript,
            symbols: vec![],
            relationships: vec![ExtractedRelationship {
                source_local_key: "widget".to_string(),
                target: EdgeTarget::Unresolved("NeverDefined".to_string()),
                kind: RelationshipKind::Injects,
                span: dummy_span,
                provenance: Provenance::Heuristic,
                confidence: Confidence::High,
                reason: Some("di:ctor-param:0".to_string()),
            }],
            unresolved: vec![],
            diagnostics: vec![],
        };
        let mut files = vec![synthetic];

        let report = resolve(&mut files, &TsconfigAliases::new());
        assert_eq!(report.unresolved, 1);
        assert!(files[0]
            .relationships
            .iter()
            .all(|r| r.kind != RelationshipKind::Injects));
        assert!(files[0]
            .unresolved
            .iter()
            .any(|r| r.relationship_kind == RelationshipKind::Injects
                && r.target_text == "NeverDefined"));
    }
}
