use codekurve_analysis::extract;
use codekurve_analysis::ir::EdgeTarget;
use codekurve_analysis::resolve::{self, TsconfigAliases};
use codekurve_core::LanguageId;

#[test]
fn mixed_languages_index_without_cross_language_edges() {
    let mut analyses = vec![
        extract::analyze(
            "export class Invoice {} export function make() { return new Invoice(); }",
            LanguageId::TypeScript,
            "shared.ts",
        )
        .unwrap(),
        extract::analyze(
            "namespace Mixed; public class Invoice {} public class Maker { public Invoice Make() => new Invoice(); }",
            LanguageId::CSharp,
            "shared.cs",
        )
        .unwrap(),
    ];
    resolve::resolve(&mut analyses, &TsconfigAliases::new());
    assert!(analyses.iter().all(|analysis| !analysis.symbols.is_empty()));
    assert!(analyses
        .iter()
        .flat_map(|analysis| &analysis.relationships)
        .all(|relationship| {
            !matches!(&relationship.target, EdgeTarget::Global { file, .. }
            if (file.ends_with(".ts") && relationship.source_local_key.ends_with(".cs"))
            || (file.ends_with(".cs") && relationship.source_local_key.ends_with(".ts")))
        }));
}
