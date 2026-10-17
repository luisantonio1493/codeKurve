//! Every symbol's storage key must be unique within its file: one duplicate
//! failed the whole index on `UNIQUE(project_id, symbol_key)`, and v0.2.11
//! could not index `dotnet/eShop`, `zod` or `effect`. Each case below is a
//! construct found in those projects that the analyzers give identical
//! (kind, qualified name, signature) tuples.

use std::collections::HashSet;

use codekurve_analysis::extract;
use codekurve_analysis::ir::FileAnalysis;
use codekurve_core::LanguageId;

fn assert_keys_unique(analysis: &FileAnalysis) {
    let mut seen = HashSet::new();
    for s in &analysis.symbols {
        assert!(
            seen.insert((
                s.kind,
                s.qualified_name.clone(),
                s.signature_fingerprint.clone(),
                s.partial_ordinal,
            )),
            "duplicate key for {} ({:?}) at line {}",
            s.qualified_name,
            s.kind,
            s.span.start_line
        );
    }
}

fn analyze(source: &str, language: LanguageId, path: &str) -> FileAnalysis {
    extract::analyze(source, language, path).unwrap()
}

/// zod `standard-schema.ts`: same interface name in two namespaces.
#[test]
fn ts_same_interface_in_two_namespaces() {
    let a = analyze(
        "export declare namespace A { interface Props { a: 1 } }\n\
         export declare namespace B { interface Props { b: 2 } }\n",
        LanguageId::TypeScript,
        "src/ns.ts",
    );
    assert_keys_unique(&a);
    assert_eq!(a.symbols.iter().filter(|s| s.name == "Props").count(), 2);
}

/// effect `TArray.ts`: declaration merging.
#[test]
fn ts_merged_interface_declarations() {
    let a = analyze(
        "export interface TArray<in out A> { a: A }\nexport interface TArray<in out A> { b: A }\n",
        LanguageId::TypeScript,
        "src/merge.ts",
    );
    assert_keys_unique(&a);
}

/// zod tests: same class name inside two callbacks.
#[test]
fn ts_same_class_in_two_callbacks() {
    let a = analyze(
        "test('a', () => { class Test {} });\ntest('b', () => { class Test {} });\n",
        LanguageId::TypeScript,
        "src/cb.test.ts",
    );
    assert_keys_unique(&a);
}

/// eShop generated gRPC code: a property plus its explicit interface
/// implementation.
#[test]
fn cs_explicit_interface_implementation() {
    let a = analyze(
        "class Req : IMessage {\n\
           public static MessageDescriptor Descriptor { get { return null; } }\n\
           MessageDescriptor IMessage.Descriptor { get { return Descriptor; } }\n\
         }\n",
        LanguageId::CSharp,
        "src/Req.cs",
    );
    assert_keys_unique(&a);
}

/// eShop `IIntegrationEventHandler.cs`: generic and non-generic interface of
/// one name, each declaring the same method signature.
#[test]
fn cs_generic_and_non_generic_interface_of_one_name() {
    let a = analyze(
        "public interface IHandler<in T> { Task Handle(IntegrationEvent @event); }\n\
         public interface IHandler { Task Handle(IntegrationEvent @event); }\n",
        LanguageId::CSharp,
        "src/IHandler.cs",
    );
    assert_keys_unique(&a);
}

/// The first occurrence keeps its original key (existing indexes keep their
/// ids); only later duplicates are renumbered, and ordinary `partial`
/// ordinals are untouched.
#[test]
fn first_occurrence_and_partials_keep_their_keys() {
    let a = analyze(
        "interface X { a: 1 }\ninterface X { b: 2 }\n",
        LanguageId::TypeScript,
        "src/x.ts",
    );
    let xs: Vec<_> = a.symbols.iter().filter(|s| s.name == "X").collect();
    assert_eq!(xs[0].partial_ordinal, None);
    assert!(xs[1].partial_ordinal.is_some());

    let c = analyze(
        "public partial class W { }\npublic partial class W { }\n",
        LanguageId::CSharp,
        "src/W.cs",
    );
    let ws: Vec<_> = c.symbols.iter().filter(|s| s.name == "W").collect();
    assert_eq!(ws[0].partial_ordinal, Some(0));
    assert_eq!(ws[1].partial_ordinal, Some(1));
}
