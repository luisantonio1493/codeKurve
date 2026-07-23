//! Fixture-driven test for PR3 intra-file relationship extraction (tasks.md
//! 3.4-3.5). Fixtures live under `tests/fixtures/ts-graph/`.

use std::fs;
use std::path::Path;

use codekurve_analysis::extract;
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
