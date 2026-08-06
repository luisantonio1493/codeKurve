//! PR7 (tasks.md 7.5): end-to-end extract + resolve over both
//! `fixtures/dotnet/` variants (controller + minimal-API) — proving the
//! minimal-API path matches the controller-based case's shape (task 7.3),
//! per-`RelationshipKind` framework edge counts, and role tags.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use codekurve_analysis::extract;
use codekurve_analysis::ir::{EdgeTarget, FileAnalysis};
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{Confidence, FrameworkRole, LanguageId, Provenance, RelationshipKind};

fn fixture(variant: &str, name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/dotnet")
        .join(variant)
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {variant}/{name}: {e}"))
}

fn analyze_variant(variant: &str, files: &[&str]) -> Vec<FileAnalysis> {
    let mut analyses: Vec<_> = files
        .iter()
        .map(|name| extract::analyze(&fixture(variant, name), LanguageId::CSharp, name).unwrap())
        .collect();
    resolve::resolve(&mut analyses, &TsconfigAliases::new());
    analyses
}

fn kind_counts(analyses: &[FileAnalysis]) -> HashMap<RelationshipKind, usize> {
    let mut counts = HashMap::new();
    for analysis in analyses {
        for rel in &analysis.relationships {
            *counts.entry(rel.kind).or_insert(0) += 1;
        }
    }
    counts
}

/// A typed feature registration (`AddDbContext<T>()`) resolves to the project
/// type its type argument names — the Category B shape.
fn assert_registered_as_target(analyses: &[FileAnalysis], simple_name: &str) {
    let found = analyses.iter().any(|a| {
        a.relationships.iter().any(|rel| {
            rel.kind == RelationshipKind::RegisteredAs
                && matches!(&rel.target, EdgeTarget::Global { qualified_name, .. }
                    if qualified_name.ends_with(simple_name))
        })
    });
    assert!(found, "no RegisteredAs edge resolved to {simple_name}");
}

/// A bare feature registration (`AddOpenApi()`) names a framework feature,
/// never a project symbol — the Category C shape is recognized, but by
/// design it survives resolution as an `UnresolvedReference`, not an edge.
fn assert_unresolved_registration(analyses: &[FileAnalysis], feature: &str) {
    let found = analyses.iter().any(|a| {
        a.unresolved.iter().any(|u| {
            u.relationship_kind == RelationshipKind::RegisteredAs && u.target_text == feature
        })
    });
    assert!(
        found,
        "no RegisteredAs UnresolvedReference recorded for {feature}"
    );
}

const CONTROLLER_FILES: &[&str] = &[
    "Invoice.cs",
    "AppDbContext.cs",
    "IInvoiceRepository.cs",
    "InvoiceRepository.cs",
    "InvoiceController.cs",
    "Startup.cs",
];

const MINIMAL_API_FILES: &[&str] = &[
    "Invoice.cs",
    "AppDbContext.cs",
    "IInvoiceRepository.cs",
    "InvoiceRepository.cs",
    "InvoiceHandlers.cs",
    "Program.cs",
];

#[test]
fn controller_variant_produces_expected_framework_edge_counts_and_roles() {
    let analyses = analyze_variant("controller", CONTROLLER_FILES);
    let counts = kind_counts(&analyses);

    // [HttpGet] on GetById -> one HandlesRoute (External route-template
    // target, terminal per D2/task 5.2).
    assert_eq!(
        counts.get(&RelationshipKind::HandlesRoute).copied(),
        Some(1)
    );
    // AddScoped<IInvoiceRepository, InvoiceRepository>() -> paired
    // RegisteredAs edges; AddDbContext<AppDbContext>() -> one more (typed
    // feature registration, target = the type argument). AddControllers()
    // also fires, but `Controllers` names no project symbol, so it is kept
    // as an UnresolvedReference rather than an edge (asserted below).
    assert_eq!(
        counts.get(&RelationshipKind::RegisteredAs).copied(),
        Some(3)
    );
    // DbSet<Invoice> in AppDbContext -> one PersistsTo from the class, one
    // more from the `DbSet<Invoice>` property itself.
    assert_eq!(counts.get(&RelationshipKind::PersistsTo).copied(), Some(2));
    assert_registered_as_target(&analyses, "AppDbContext");
    assert_unresolved_registration(&analyses, "Controllers");

    let controller = analyses
        .iter()
        .find(|a| a.file == "InvoiceController.cs")
        .unwrap();
    assert!(controller
        .symbols
        .iter()
        .any(|s| s.name == "InvoiceController" && s.roles.contains(&FrameworkRole::Controller)));
    assert!(controller
        .symbols
        .iter()
        .any(|s| s.name == "GetById" && s.roles.contains(&FrameworkRole::Route)));
}

#[test]
fn minimal_api_variant_matches_the_controller_variants_shape() {
    let analyses = analyze_variant("minimal-api", MINIMAL_API_FILES);
    let counts = kind_counts(&analyses);

    // MapGet(...) -> External template edge + Unresolved(handler) edge that
    // resolves to InvoiceHandlers.GetInvoice = 2.
    assert_eq!(
        counts.get(&RelationshipKind::HandlesRoute).copied(),
        Some(2)
    );
    // Same shape as the controller variant: the AddScoped pair plus the
    // AddDbContext<AppDbContext> typed feature registration.
    assert_eq!(
        counts.get(&RelationshipKind::RegisteredAs).copied(),
        Some(3)
    );
    assert_eq!(counts.get(&RelationshipKind::PersistsTo).copied(), Some(2));
    assert_registered_as_target(&analyses, "AppDbContext");
    assert_unresolved_registration(&analyses, "OpenApi");

    let program = analyses.iter().find(|a| a.file == "Program.cs").unwrap();
    let resolves_to_handler = program.relationships.iter().any(|rel| {
        rel.kind == RelationshipKind::HandlesRoute
            && matches!(&rel.target, EdgeTarget::Global { qualified_name, .. } if qualified_name.ends_with("GetInvoice"))
    });
    assert!(
        resolves_to_handler,
        "MapGet handler edge did not resolve to InvoiceHandlers.GetInvoice"
    );
}

/// Task 7.6: no framework edge is ever `extracted`/`resolved`, across both
/// fixture variants in full.
#[test]
fn no_dotnet_framework_edge_is_ever_extracted_or_resolved() {
    let framework_kinds = [
        RelationshipKind::HandlesRoute,
        RelationshipKind::RegisteredAs,
        RelationshipKind::Triggers,
        RelationshipKind::PersistsTo,
    ];
    for (variant, files) in [
        ("controller", CONTROLLER_FILES),
        ("minimal-api", MINIMAL_API_FILES),
    ] {
        let analyses = analyze_variant(variant, files);
        let mut seen_any = false;
        for analysis in &analyses {
            for rel in &analysis.relationships {
                if framework_kinds.contains(&rel.kind) {
                    seen_any = true;
                    assert_eq!(rel.provenance, Provenance::Heuristic, "{variant}: {rel:?}");
                    assert_ne!(rel.confidence, Confidence::Exact, "{variant}: {rel:?}");
                }
            }
        }
        assert!(seen_any, "{variant} produced no framework edges");
    }
}

/// Task 7.7: a plain TS + plain C# project produces zero framework edges
/// and zero roles (reuse `fixtures/mixed/` from phase 5).
#[test]
fn mixed_fixture_produces_zero_framework_edges_and_roles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/mixed/src");
    let ts = fs::read_to_string(root.join("shared.ts")).unwrap();
    let cs = fs::read_to_string(root.join("shared.cs")).unwrap();
    let mut analyses = vec![
        extract::analyze(&ts, LanguageId::TypeScript, "shared.ts").unwrap(),
        extract::analyze(&cs, LanguageId::CSharp, "shared.cs").unwrap(),
    ];
    resolve::resolve(&mut analyses, &TsconfigAliases::new());

    let framework_kinds = [
        RelationshipKind::Injects,
        RelationshipKind::RegisteredAs,
        RelationshipKind::HandlesRoute,
        RelationshipKind::Triggers,
        RelationshipKind::PersistsTo,
    ];
    for analysis in &analyses {
        assert!(analysis
            .relationships
            .iter()
            .all(|rel| !framework_kinds.contains(&rel.kind)));
        assert!(analysis.symbols.iter().all(|s| s.roles.is_empty()));
    }
}
