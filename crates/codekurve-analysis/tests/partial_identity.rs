use codekurve_analysis::extract;
use codekurve_core::LanguageId;
use codekurve_store::repo::symbol_key;

#[test]
fn partial_fragments_keep_distinct_identity_while_non_partial_stays_unset() {
    let same_file = extract::analyze(
        "partial class Invoice {} partial class Invoice {} class Plain {}",
        LanguageId::CSharp,
        "invoice.cs",
    )
    .unwrap();
    let partials: Vec<_> = same_file
        .symbols
        .iter()
        .filter(|symbol| symbol.is_partial)
        .collect();
    assert_eq!(partials.len(), 2);
    assert_ne!(partials[0].partial_ordinal, partials[1].partial_ordinal);
    let plain = same_file
        .symbols
        .iter()
        .find(|symbol| symbol.name == "Plain")
        .unwrap();
    assert!(plain.partial_ordinal.is_none());
    assert_eq!(
        symbol_key(
            "typescript",
            "src/member.ts",
            "class",
            "src/member.ts::MemberService",
            "",
            None
        ),
        "8be6b5b411dfdc32b2948f30e91dab5251ad60480b1cd3cddf250f5c1d4470af"
    );
    for file in ["a.cs", "b.cs"] {
        let analysis =
            extract::analyze("partial class Invoice {}", LanguageId::CSharp, file).unwrap();
        assert_eq!(analysis.symbols[0].partial_ordinal, Some(0));
    }
}
