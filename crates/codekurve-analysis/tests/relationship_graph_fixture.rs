//! Acceptance gate for PR4b (tasks.md 4b.5, design §34.2-34.3): runs
//! `extract::analyze` + `resolve::resolve` over a small multi-file TS
//! project fixture and asserts the resolved graph — relationship counts by
//! kind, one specific cross-file edge, and that an intentionally-unresolved
//! reference is recorded rather than dropped (§18.3).
//!
//! `RelationshipKind::References` is never emitted by `extract::analyze` in
//! this phase (no code path produces it yet) — not exercised here.
//! `Implements` is deliberately never satisfied by this fixture either: no
//! `Interface` symbols are extracted anywhere in this phase (a pre-existing
//! gap, not introduced by PR4b), so `implements IFoo` doubles as this
//! fixture's "real" unresolved-reference scenario rather than a known-gap
//! path (default export / namespace import) being relied on.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use codekurve_analysis::extract;
use codekurve_analysis::ir::EdgeTarget;
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{Confidence, LanguageId, Provenance, RelationshipKind};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ts-graph/project")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

#[test]
fn multi_file_project_resolves_across_files() {
    let names = ["base.ts", "utils.ts", "app.ts"];
    let mut analyses: Vec<_> = names
        .iter()
        .map(|name| extract::analyze(&fixture(name), LanguageId::TypeScript, name).unwrap())
        .collect();

    let report = resolve::resolve(&mut analyses, &TsconfigAliases::new());

    let mut counts: HashMap<RelationshipKind, usize> = HashMap::new();
    for analysis in &analyses {
        for rel in &analysis.relationships {
            *counts.entry(rel.kind).or_default() += 1;
        }
    }

    assert_eq!(counts.get(&RelationshipKind::Contains), Some(&3));
    assert_eq!(counts.get(&RelationshipKind::Exports), Some(&3));
    assert_eq!(counts.get(&RelationshipKind::Inherits), Some(&1));
    assert_eq!(counts.get(&RelationshipKind::Calls), Some(&2));
    assert_eq!(counts.get(&RelationshipKind::Constructs), Some(&1));
    assert_eq!(counts.get(&RelationshipKind::Imports), Some(&2));
    // `implements IFoo` never resolves to a `relationships` row (see module
    // doc comment) — it lands in `unresolved` instead, asserted below.
    assert!(!counts.contains_key(&RelationshipKind::Implements));

    // A specific cross-file edge: `helper()` called from `app.ts::Foo.run`
    // resolves to `utils.ts`'s exported `helper` function.
    let app = analyses.iter().find(|a| a.file == "app.ts").unwrap();
    let cross_file_call = app
        .relationships
        .iter()
        .find(|r| {
            r.kind == RelationshipKind::Calls
                && matches!(&r.target, EdgeTarget::Global { qualified_name, .. }
                    if qualified_name == "utils.ts::helper")
        })
        .expect("cross-file call to utils.ts::helper");
    assert_eq!(
        cross_file_call.target,
        EdgeTarget::Global {
            file: "utils.ts".to_string(),
            qualified_name: "utils.ts::helper".to_string(),
        }
    );
    assert_eq!(cross_file_call.provenance, Provenance::Resolved);
    assert_eq!(cross_file_call.confidence, Confidence::High);

    // Unresolved references are recorded, never dropped (§18.3): the
    // `implements IFoo` heritage clause and the `./nonexistent` import.
    let unresolved: Vec<_> = analyses.iter().flat_map(|a| a.unresolved.iter()).collect();
    assert_eq!(unresolved.len(), 2);
    assert!(unresolved
        .iter()
        .any(|u| u.relationship_kind == RelationshipKind::Implements
            && u.target_text == "IFoo"
            && u.candidate_count == 0));
    assert!(unresolved
        .iter()
        .any(|u| u.relationship_kind == RelationshipKind::Imports
            && u.target_text == "./nonexistent"
            && u.candidate_count == 0));

    assert_eq!(report.resolved, 5);
    assert_eq!(report.unresolved, 2);
}
