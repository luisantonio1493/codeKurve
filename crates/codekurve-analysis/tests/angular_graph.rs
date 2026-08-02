//! PR7 (tasks.md 7.4): end-to-end extract + resolve over `fixtures/angular/`
//! — the route -> component -> injected-service chain (proposal Success
//! Criteria, framework-awareness "An End-to-End Route-to-Data-Layer Path Is
//! Traversable"). Asserts exact per-`RelationshipKind` framework edge counts
//! and every role tag, not just "some edge exists".

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use codekurve_analysis::extract;
use codekurve_analysis::ir::EdgeTarget;
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::{Confidence, FrameworkRole, LanguageId, Provenance, RelationshipKind};

const FILES: &[&str] = &[
    "invoice-api.repository.ts",
    "auth-interceptor.ts",
    "invoice.component.ts",
    "invoice-list.component.ts",
    "invoice-lazy.component.ts",
    "shared.module.ts",
    "app.module.ts",
    "app.routes.ts",
    "auth.guard.ts",
];

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/angular/src")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

fn analyze_fixture() -> Vec<codekurve_analysis::ir::FileAnalysis> {
    let mut analyses: Vec<_> = FILES
        .iter()
        .map(|name| extract::analyze(&fixture(name), LanguageId::TypeScript, name).unwrap())
        .collect();
    resolve::resolve(&mut analyses, &TsconfigAliases::new());
    analyses
}

#[test]
fn angular_fixture_produces_expected_framework_edge_counts() {
    let analyses = analyze_fixture();
    let mut counts: HashMap<RelationshipKind, usize> = HashMap::new();
    for analysis in &analyses {
        for rel in &analysis.relationships {
            *counts.entry(rel.kind).or_insert(0) += 1;
        }
    }

    // Injects: constructor-param DI (InvoiceComponent, InvoiceLazyComponent)
    // + `inject()` DI (InvoiceListComponent) -> InvoiceApiRepository each.
    assert_eq!(counts.get(&RelationshipKind::Injects).copied(), Some(3));

    // RegisteredAs: NgModule `imports: [SharedModule]`,
    // `providers: [InvoiceApiRepository, { HTTP_INTERCEPTORS }]`, standalone
    // `imports: [SharedModule]` on InvoiceComponent, `canActivate: [AuthGuard]`
    // on the routes array = 5.
    assert_eq!(
        counts.get(&RelationshipKind::RegisteredAs).copied(),
        Some(5)
    );

    // HandlesRoute: `''` -> InvoiceListComponent, `:id` -> InvoiceComponent,
    // `:id/lazy` -> InvoiceLazyComponent (loadComponent) = 3.
    assert_eq!(
        counts.get(&RelationshipKind::HandlesRoute).copied(),
        Some(3)
    );
}

#[test]
fn angular_fixture_tags_every_expected_role() {
    let analyses = analyze_fixture();
    let role_of = |name: &str| -> Vec<FrameworkRole> {
        analyses
            .iter()
            .flat_map(|a| &a.symbols)
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol named {name}"))
            .roles
            .clone()
    };

    assert!(role_of("InvoiceApiRepository").contains(&FrameworkRole::Service));
    assert!(role_of("InvoiceApiRepository").contains(&FrameworkRole::Repository));
    assert!(role_of("AuthInterceptor").contains(&FrameworkRole::Service));
    assert!(role_of("InvoiceComponent").contains(&FrameworkRole::Component));
    assert!(role_of("InvoiceListComponent").contains(&FrameworkRole::Component));
}

#[test]
fn angular_fixture_route_to_component_edge_resolves_to_a_real_symbol() {
    let analyses = analyze_fixture();
    let routes = analyses.iter().find(|a| a.file == "app.routes.ts").unwrap();
    let resolved_to_invoice_component = routes.relationships.iter().any(|rel| {
        rel.kind == RelationshipKind::HandlesRoute
            && matches!(&rel.target, EdgeTarget::Global { qualified_name, .. } if qualified_name.ends_with("InvoiceComponent"))
    });
    assert!(
        resolved_to_invoice_component,
        "route -> InvoiceComponent edge did not resolve to a real symbol"
    );
}

#[test]
fn angular_fixture_component_to_service_injects_edge_resolves() {
    let analyses = analyze_fixture();
    let component = analyses
        .iter()
        .find(|a| a.file == "invoice.component.ts")
        .unwrap();
    let injects_repo = component.relationships.iter().any(|rel| {
        rel.kind == RelationshipKind::Injects
            && matches!(&rel.target, EdgeTarget::Global { qualified_name, .. } if qualified_name.ends_with("InvoiceApiRepository"))
    });
    assert!(
        injects_repo,
        "component -> repository Injects edge did not resolve"
    );
}

/// Task 7.6: no framework edge is ever `extracted`/`resolved` — D5's
/// provenance floor holds even when the edge fully resolves to a real
/// symbol (as several of this fixture's edges do).
#[test]
fn no_angular_framework_edge_is_ever_extracted_or_resolved() {
    let analyses = analyze_fixture();
    let framework_kinds = [
        RelationshipKind::Injects,
        RelationshipKind::RegisteredAs,
        RelationshipKind::HandlesRoute,
    ];
    let mut seen_any = false;
    for analysis in &analyses {
        for rel in &analysis.relationships {
            if framework_kinds.contains(&rel.kind) {
                seen_any = true;
                assert_eq!(rel.provenance, Provenance::Heuristic, "{rel:?}");
                assert_ne!(rel.confidence, Confidence::Exact, "{rel:?}");
            }
        }
    }
    assert!(seen_any);
}
