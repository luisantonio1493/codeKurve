//! Acceptance gate for PR4b (tasks.md 4b.5, design §34.2-34.3): runs
//! `extract::analyze` + `resolve::resolve` over a small multi-file TS
//! project fixture and asserts the resolved graph — relationship counts by
//! kind, one specific cross-file edge, and that an intentionally-unresolved
//! reference is recorded rather than dropped (§18.3).
//!
//! `base.ts` declares `IFoo` as a real interface, so `app.ts`'s
//! `implements IFoo` resolves to a `Resolved`-provenance edge, matching spec
//! scenario "Class extends and implements" (fixed post-verify remediation —
//! previously this fixture asserted the opposite, that `Implements` never
//! resolves, contradicting that scenario). `app.ts::build`'s `: Base` return
//! type annotation also exercises the `References` kind, resolving
//! cross-file to `base.ts::Base`. The only remaining unresolved reference is
//! `./nonexistent` (spec "Zero-candidate import").

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
    // `implements IFoo` now resolves — `base.ts` declares a real `IFoo`
    // interface (spec "Class extends and implements", in-project case).
    assert_eq!(counts.get(&RelationshipKind::Implements), Some(&1));
    // `build(): Base` return type annotation (spec "Relationship Kind
    // Extraction" — `references` is a MUST-extracted kind).
    assert_eq!(counts.get(&RelationshipKind::References), Some(&1));

    let app = analyses.iter().find(|a| a.file == "app.ts").unwrap();
    let implements = app
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::Implements)
        .expect("implements edge resolves now that IFoo is a real interface");
    assert_eq!(
        implements.target,
        EdgeTarget::Global {
            file: "base.ts".to_string(),
            qualified_name: "base.ts::IFoo".to_string(),
        }
    );
    assert_eq!(implements.provenance, Provenance::Resolved);
    assert_eq!(implements.confidence, Confidence::High);

    let references = app
        .relationships
        .iter()
        .find(|r| r.kind == RelationshipKind::References)
        .expect("build()'s return type resolves to base.ts::Base");
    assert_eq!(
        references.target,
        EdgeTarget::Global {
            file: "base.ts".to_string(),
            qualified_name: "base.ts::Base".to_string(),
        }
    );
    assert_eq!(references.provenance, Provenance::Resolved);
    assert_eq!(references.confidence, Confidence::High);

    // A specific cross-file edge: `helper()` called from `app.ts::Foo.run`
    // resolves to `utils.ts`'s exported `helper` function.
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

    // Unresolved references are recorded, never dropped (§18.3): now only
    // the `./nonexistent` import — `implements IFoo` resolves above.
    let unresolved: Vec<_> = analyses.iter().flat_map(|a| a.unresolved.iter()).collect();
    assert_eq!(unresolved.len(), 1);
    assert!(unresolved
        .iter()
        .any(|u| u.relationship_kind == RelationshipKind::Imports
            && u.target_text == "./nonexistent"
            && u.candidate_count == 0));

    assert_eq!(report.resolved, 7);
    assert_eq!(report.unresolved, 1);
}
