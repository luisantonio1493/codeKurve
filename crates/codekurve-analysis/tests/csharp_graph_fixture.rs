use std::collections::HashMap;
use std::fs;
use std::path::Path;

use codekurve_analysis::extract;
use codekurve_analysis::ir::EdgeTarget;
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{Confidence, LanguageId, Provenance, RelationshipKind};

fn fixture(name: &str) -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/csharp-graph/project")
            .join(name),
    )
    .unwrap()
}

#[test]
fn csharp_graph_fixture_resolves_cross_file_relationships() {
    let mut analyses: Vec<_> = ["contracts.cs", "invoice.cs", "invoice.partial.cs"]
        .iter()
        .map(|name| extract::analyze(&fixture(name), LanguageId::CSharp, name).unwrap())
        .collect();
    let report = resolve::resolve(&mut analyses, &TsconfigAliases::new());
    let mut counts = HashMap::new();
    for analysis in &analyses {
        for rel in &analysis.relationships {
            *counts.entry(rel.kind).or_insert(0usize) += 1;
        }
    }
    for kind in [
        RelationshipKind::Inherits,
        RelationshipKind::Implements,
        RelationshipKind::Calls,
        RelationshipKind::Constructs,
        RelationshipKind::Imports,
        RelationshipKind::Decorates,
    ] {
        assert!(
            counts.get(&kind).copied().unwrap_or_default() > 0,
            "missing {kind:?}"
        );
    }
    let invoice = analyses
        .iter()
        .find(|analysis| analysis.file == "invoice.cs")
        .unwrap();
    for (kind, target) in [
        (
            RelationshipKind::Inherits,
            "contracts.cs::Acme.Contracts.InvoiceBase",
        ),
        (
            RelationshipKind::Implements,
            "contracts.cs::Acme.Contracts.IInvoiceProcessor",
        ),
        (
            RelationshipKind::Calls,
            "contracts.cs::Acme.Contracts.InvoiceBase.Calculate",
        ),
        (
            RelationshipKind::Constructs,
            "contracts.cs::Acme.Contracts.InvoiceBase",
        ),
        (RelationshipKind::Imports, "contracts.cs::Acme.Contracts"),
    ] {
        assert!(invoice.relationships.iter().any(|rel| {
            rel.kind == kind
                && matches!(&rel.target, EdgeTarget::Global { qualified_name, .. } if qualified_name == target)
                && rel.provenance == Provenance::Resolved
                && rel.confidence == Confidence::High
        }), "missing resolved {kind:?} -> {target}");
    }
    assert!(invoice
        .relationships
        .iter()
        .any(|rel| rel.kind == RelationshipKind::Decorates));
    assert!(analyses
        .iter()
        .flat_map(|analysis| &analysis.unresolved)
        .any(|unresolved| { unresolved.target_text == "Object" && !unresolved.reason.is_empty() }));
    assert!(report.resolved >= 5);
}

#[test]
fn csharp_single_file_cases_cover_declared_surface() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/csharp-graph");
    let visibility = extract::analyze(
        &fs::read_to_string(root.join("visibility.cs")).unwrap(),
        LanguageId::CSharp,
        "visibility.cs",
    )
    .unwrap();
    for expected_visibility in [
        "public",
        "protected",
        "internal",
        "private",
        "protectedinternal",
        "privateprotected",
    ] {
        assert!(visibility
            .symbols
            .iter()
            .any(|symbol| symbol.visibility.as_str() == expected_visibility));
    }
    let block = extract::analyze(
        &fs::read_to_string(root.join("block_namespace.cs")).unwrap(),
        LanguageId::CSharp,
        "block_namespace.cs",
    )
    .unwrap();
    assert!(block
        .symbols
        .iter()
        .any(|symbol| symbol.name == "Acme.Block"));
    assert!(block
        .symbols
        .iter()
        .any(|symbol| symbol.qualified_name.ends_with("Outer.Inner")));
    let records = extract::analyze(
        &fs::read_to_string(root.join("records.cs")).unwrap(),
        LanguageId::CSharp,
        "records.cs",
    )
    .unwrap();
    assert_eq!(
        records
            .symbols
            .iter()
            .filter(|symbol| symbol.is_record)
            .count(),
        2
    );
    assert!(records.symbols.iter().any(|symbol| symbol.name == "Open"));
    let target_typed = extract::analyze(
        &fs::read_to_string(root.join("target_typed_new.cs")).unwrap(),
        LanguageId::CSharp,
        "target_typed_new.cs",
    )
    .unwrap();
    assert!(target_typed
        .relationships
        .iter()
        .any(
            |relationship| relationship.kind == RelationshipKind::Constructs
                && relationship.reason.as_deref()
                    == Some("target-typed new() has no type name at the call site")
        ));
}
