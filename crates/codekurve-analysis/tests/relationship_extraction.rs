//! Fixture-driven test for PR3 intra-file relationship extraction (tasks.md
//! 3.4-3.5). Fixtures live under `tests/fixtures/ts-graph/`.

use std::fs;
use std::path::Path;

use codekurve_analysis::extract;
use codekurve_analysis::ir::EdgeTarget;
use codekurve_core::{Confidence, LanguageId, Provenance, RelationshipKind};

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ts-graph")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

/// Spec scenario "Class extends and implements" (Requirement "Relationship
/// Kind Extraction").
#[test]
fn heritage_fixture_extends_and_implements() {
    let source = fixture("heritage.ts");
    let analysis = extract::analyze(&source, LanguageId::TypeScript, "heritage.ts").unwrap();

    let extends = analysis
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::Inherits)
        .unwrap();
    assert_eq!(extends.source_local_key, "heritage.ts::Foo");
    assert_eq!(extends.provenance, Provenance::Extracted);
    assert_eq!(extends.confidence, Confidence::Exact);

    let implements = analysis
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::Implements)
        .unwrap();
    assert_eq!(implements.source_local_key, "heritage.ts::Foo");
    assert_eq!(implements.provenance, Provenance::Extracted);
}

/// Spec scenario "Contains hierarchy" (Requirement "Relationship Kind
/// Extraction").
#[test]
fn contains_fixture_links_class_to_methods() {
    let source = fixture("contains.ts");
    let analysis = extract::analyze(&source, LanguageId::TypeScript, "contains.ts").unwrap();

    let contains: Vec<_> = analysis
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Contains)
        .collect();
    assert_eq!(contains.len(), 2);
    assert!(contains
        .iter()
        .all(|r| r.source_local_key == "contains.ts::Box"));
}

/// Task 3.2 (imports/exports edges) — named/default/namespace imports and
/// named/default/re-export statements, per §20.1's "imports"/"exports"
/// relationship kinds. All targets stay `Unresolved` at extraction time;
/// module resolution to concrete files/symbols is a later phase slice
/// (PR4a).
#[test]
fn imports_exports_fixture() {
    let source = fixture("imports-exports.ts");
    let analysis = extract::analyze(&source, LanguageId::TypeScript, "imports-exports.ts").unwrap();

    let imports: Vec<_> = analysis
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Imports)
        .collect();
    // default (Widget), namespace (* as utils), 2 named (helper, other as renamed)
    assert_eq!(imports.len(), 4);
    for edge in &imports {
        assert_eq!(edge.source_local_key, "imports-exports.ts");
        assert_eq!(edge.provenance, Provenance::Extracted);
        assert_eq!(edge.confidence, Confidence::Unresolved);
    }
    assert!(imports.iter().any(
        |r| r.target == EdgeTarget::Unresolved("./widget".to_string())
            && r.reason.as_deref() == Some("default")
    ));
    assert!(imports.iter().any(
        |r| r.target == EdgeTarget::Unresolved("./utils".to_string())
            && r.reason.as_deref() == Some("*")
    ));
    assert!(imports.iter().any(
        |r| r.target == EdgeTarget::Unresolved("./helpers".to_string())
            && r.reason.as_deref() == Some("helper")
    ));
    // `other as renamed` — the target-module export name (`other`) is kept,
    // the local alias is dropped (extraction, not resolution).
    assert!(imports.iter().any(
        |r| r.target == EdgeTarget::Unresolved("./helpers".to_string())
            && r.reason.as_deref() == Some("other")
    ));

    let exports: Vec<_> = analysis
        .relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Exports)
        .collect();
    // `export { Box, helper }` (2 edges: Box local-resolves, helper is an
    // import binding so stays Unresolved) + `export default Box`
    // (local-resolves) + `export { other } from './external'`
    // + `export * from './everything'`
    assert_eq!(exports.len(), 5);

    // `export { Box, helper }` + `export default Box` both resolve `Box`
    // locally — two distinct edges, both Exact.
    let box_edges: Vec<_> = exports
        .iter()
        .filter(|r| matches!(&r.target, EdgeTarget::Local(k) if k == "imports-exports.ts::Box"))
        .collect();
    assert_eq!(box_edges.len(), 2, "export {{ Box }} + export default Box");
    assert!(box_edges.iter().all(|r| r.confidence == Confidence::Exact));

    let helper_export = exports
        .iter()
        .find(|r| r.target == EdgeTarget::Unresolved("helper".to_string()))
        .expect("helper named export has no local symbol");
    assert_eq!(helper_export.confidence, Confidence::Unresolved);

    assert!(exports.iter().any(
        |r| r.target == EdgeTarget::Unresolved("./external".to_string())
            && r.reason.as_deref() == Some("other")
    ));
    assert!(exports.iter().any(
        |r| r.target == EdgeTarget::Unresolved("./everything".to_string()) && r.reason.is_none()
    ));
}
