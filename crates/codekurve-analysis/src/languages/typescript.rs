//! TypeScript/JavaScript extraction — moved verbatim from `extract.rs`
//! behind `impl LanguageAnalyzer for TypeScriptAnalyzer` (design "Module
//! Layout", "Helper split"; Phase 5 PR2, behavior-preserving move, no logic
//! edits). See CODEKURVE_MASTER_PLAN.md §Fase 1 (symbols) and §18/§20
//! (relationship IR). Cross-file resolution lives in `resolve.rs`; every
//! edge here either resolves to a same-file symbol or carries
//! `EdgeTarget::Unresolved(text)` for that later pass to pick up.

use codekurve_core::error::{Error, Result};
use codekurve_core::{Confidence, LanguageId, Provenance, RelationshipKind, SymbolKind};
use tree_sitter::{Node, Parser};

use crate::extract::{find_child, span_of};
use crate::ir::{EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis};
use crate::languages::{
    analyzer_for, push_unresolved_edge, resolve_pending, LanguageAnalyzer, PendingRel,
};

/// One-field struct so a single implementation serves both TypeScript and
/// JavaScript — `analyze` has no `language` parameter of its own, the
/// grammar is picked from `self.language` (design "Module Layout").
pub struct TypeScriptAnalyzer {
    language: LanguageId,
}

pub(crate) const TS: TypeScriptAnalyzer = TypeScriptAnalyzer {
    language: LanguageId::TypeScript,
};
pub(crate) const JS: TypeScriptAnalyzer = TypeScriptAnalyzer {
    language: LanguageId::JavaScript,
};

impl LanguageAnalyzer for TypeScriptAnalyzer {
    fn language(&self) -> LanguageId {
        self.language
    }

    fn analyze(&self, source: &str, relative_path: &str) -> Result<FileAnalysis> {
        analyze(source, self.language, relative_path)
    }

    /// Today's `extract::kind_matches` body, moved verbatim — that identity
    /// is the no-regression guarantee (design "Resolution Changes").
    fn kind_matches(&self, rel_kind: RelationshipKind, sym_kind: SymbolKind) -> bool {
        match rel_kind {
            RelationshipKind::Constructs => sym_kind == SymbolKind::Class,
            RelationshipKind::Calls => matches!(
                sym_kind,
                SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
            ),
            RelationshipKind::Inherits | RelationshipKind::Implements => {
                matches!(sym_kind, SymbolKind::Class | SymbolKind::Interface)
            }
            RelationshipKind::References => {
                matches!(
                    sym_kind,
                    SymbolKind::Class | SymbolKind::Interface | SymbolKind::TypeAlias
                )
            }
            RelationshipKind::Exports => {
                matches!(
                    sym_kind,
                    SymbolKind::Class | SymbolKind::Function | SymbolKind::Interface
                )
            }
            // Task 2.4/spec "TypeScript Kind Matching Extends to Decorates
            // Without TypeScript Regression": accepts `Decorates` the same
            // unconditional way `CSharpAnalyzer::kind_matches` does (its own
            // `_ => true` catch-all, csharp.rs) — every non-`Decorates`
            // answer above is untouched.
            _ => true,
        }
    }
}

/// Parse `source` for the given language and extract its symbols plus
/// intra-file relationships into a per-file `FileAnalysis`. `unresolved`
/// stays empty here — it is populated once whole-project resolution proves a
/// target has zero candidates anywhere in the project.
fn analyze(source: &str, language: LanguageId, relative_path: &str) -> Result<FileAnalysis> {
    // ponytail: one grammar for both languages — the TypeScript grammar parses
    // plain JS as a subset, TSX covers JS/JSX. Add tree-sitter-javascript only
    // if fidelity gaps appear (§12 lists it for later).
    let grammar = match language {
        LanguageId::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        LanguageId::JavaScript => tree_sitter_typescript::LANGUAGE_TSX,
        LanguageId::CSharp => unreachable!("TypeScriptAnalyzer only serves TS/JS"),
    };

    let mut parser = Parser::new();
    parser
        .set_language(&grammar.into())
        .map_err(|e| Error::Parse(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse("parser returned no tree".to_string()))?;

    let mut ctx = CollectCtx {
        source: source.as_bytes(),
        language,
        relative_path,
        out: Vec::new(),
        out_rels: Vec::new(),
        pending: Vec::new(),
    };
    collect(tree.root_node(), None, None, &mut ctx);
    let mut relationships = ctx.out_rels;
    resolve_pending(
        &ctx.out,
        ctx.pending,
        &mut relationships,
        analyzer_for(language),
    );

    Ok(FileAnalysis {
        file: relative_path.to_string(),
        language,
        symbols: ctx.out,
        relationships,
        unresolved: Vec::new(),
        diagnostics: Vec::new(),
    })
}

/// Everything the recursive walk needs that doesn't change with tree depth —
/// bundled so `collect` stays under clippy's argument-count limit; `node`,
/// `parent` (enclosing class name, for qualified names), and `scope`
/// (enclosing function/method local key, for call/construct attribution)
/// are the only things that vary per call and stay as explicit arguments.
struct CollectCtx<'a> {
    source: &'a [u8],
    language: LanguageId,
    relative_path: &'a str,
    out: Vec<ExtractedSymbol>,
    out_rels: Vec<ExtractedRelationship>,
    pending: Vec<PendingRel>,
}

fn collect(node: Node, parent: Option<&str>, scope: Option<&str>, ctx: &mut CollectCtx) {
    let (source, language, relative_path) = (ctx.source, ctx.language, ctx.relative_path);
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            let name = push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Class,
                parent,
                &mut ctx.out,
            );
            if let Some(class_name) = &name {
                collect_heritage(node, source, relative_path, class_name, &mut ctx.pending);
                // Class-level decorators (`@Component`, `@Injectable`, ...)
                // are a `decorator` field on `class_declaration` itself, not
                // a child of `class_body` (task 2.2/2.5, D15's grammar note).
                let class_key = qualified_name(relative_path, None, class_name);
                let mut dcursor = node.walk();
                for decorator in node.children_by_field_name("decorator", &mut dcursor) {
                    push_decorator_edge(&mut ctx.out_rels, &class_key, decorator, source, None);
                }
            }
            // Descend with the class name as the new parent so members get a
            // `Class.member` qualified name; this replaces the generic
            // recursion below for this subtree.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, name.as_deref(), scope, ctx);
            }
            return;
        }
        // Member decorators split into two grammar shapes (task 2.1's
        // node-types.json check, verified against an actual parse — the
        // per-type `fields` listing alone is misleading here): a
        // `public_field_definition` (property) carries its own `decorator`
        // field directly, but a `method_definition`'s decorator is *not* a
        // field of the method — it surfaces as a `decorator` field on
        // `class_body` itself, immediately preceding the method it
        // annotates. Both are handled here so the shared recursion below
        // still runs unmodified for every child.
        "class_body" => {
            let mut cursor = node.walk();
            let children: Vec<Node> = node.named_children(&mut cursor).collect();
            let mut pending_decorators: Vec<Node> = Vec::new();
            for child in children {
                if child.kind() == "decorator" {
                    pending_decorators.push(child);
                    continue;
                }
                if let Some(member_name) = member_name_text(child, source) {
                    let member_key = qualified_name(relative_path, parent, &member_name);
                    match child.kind() {
                        "method_definition" => {
                            for decorator in &pending_decorators {
                                push_decorator_edge(
                                    &mut ctx.out_rels,
                                    &member_key,
                                    *decorator,
                                    source,
                                    None,
                                );
                            }
                        }
                        "public_field_definition" => {
                            let mut pcursor = child.walk();
                            for decorator in child.children_by_field_name("decorator", &mut pcursor)
                            {
                                push_decorator_edge(
                                    &mut ctx.out_rels,
                                    &member_key,
                                    decorator,
                                    source,
                                    None,
                                );
                            }
                        }
                        _ => {}
                    }
                }
                pending_decorators.clear();
                collect(child, parent, scope, ctx);
            }
            return;
        }
        // A raw `decorator` node reached outside the anchor points above
        // (e.g. a `required_parameter`'s `decorator` field, visited via the
        // shared recursion below) is never walked generically — its inner
        // `call_expression` would otherwise be picked up by the
        // `call_expression` arm and misread as an ordinary call.
        "decorator" => return,
        "interface_declaration" => {
            // Falls through to the shared recursion below (no early
            // `return`) — interface members (`property_signature`/
            // `method_signature`) aren't extracted as symbols in this phase,
            // only the interface itself, so it can be a resolution target
            // for `implements` clauses (spec "Class extends and implements").
            push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Interface,
                parent,
                &mut ctx.out,
            );
        }
        "method_definition" if parent.is_some() => {
            let Some(class_name) = parent else { return };
            let kind = method_kind(node, source);
            let Some(method_name) = push_named(
                node,
                source,
                language,
                relative_path,
                kind,
                parent,
                &mut ctx.out,
            ) else {
                return;
            };
            let class_key = qualified_name(relative_path, None, class_name);
            let method_key = qualified_name(relative_path, Some(class_name), &method_name);
            ctx.out_rels.push(ExtractedRelationship {
                source_local_key: class_key,
                target: EdgeTarget::Local(method_key.clone()),
                kind: RelationshipKind::Contains,
                span: span_of(node),
                provenance: Provenance::Extracted,
                confidence: Confidence::Exact,
                reason: None,
            });
            // Constructor-parameter decorators (`@Inject(TOKEN)`) — TS has no
            // parameter symbols, so the source is the constructor itself
            // (D15); `reason = "param:<index>"` is what lets a later pass
            // match the decorator back to its parameter position. Only
            // constructors get this treatment (task 2.3) — an ordinary
            // method's parameter decorators are out of this PR's scope.
            if kind == SymbolKind::Constructor {
                collect_constructor_param_decorators(node, &method_key, source, &mut ctx.out_rels);
            }
            // A method body can contain its own nested functions/classes;
            // recurse into it without the enclosing class as parent, but with
            // this method as the call/construct attribution scope.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, None, Some(method_key.as_str()), ctx);
            }
            return;
        }
        "function_declaration" | "generator_function_declaration" if is_top_level(node) => {
            let Some(name) = push_named(
                node,
                source,
                language,
                relative_path,
                SymbolKind::Function,
                None,
                &mut ctx.out,
            ) else {
                return;
            };
            let key = qualified_name(relative_path, None, &name);
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, parent, Some(key.as_str()), ctx);
            }
            return;
        }
        "import_statement" => {
            collect_imports(node, source, relative_path, &mut ctx.out_rels);
        }
        "export_statement" => {
            collect_exports(
                node,
                source,
                relative_path,
                &mut ctx.out_rels,
                &mut ctx.pending,
            );
            // `export class Foo {}` moves the class's own decorator(s) onto
            // `export_statement`'s `decorator` field rather than
            // `class_declaration`'s (task 2.1 — verified against an actual
            // parse, not just node-types.json's per-type field listing).
            // Falls through to the shared recursion below, which still
            // visits `declaration` normally for symbol/heritage extraction.
            if let Some(decl) = node.child_by_field_name("declaration") {
                if matches!(
                    decl.kind(),
                    "class_declaration" | "abstract_class_declaration"
                ) {
                    if let Some(class_name) = member_name_text(decl, source) {
                        let class_key = qualified_name(relative_path, None, &class_name);
                        let mut dcursor = node.walk();
                        for decorator in node.children_by_field_name("decorator", &mut dcursor) {
                            push_decorator_edge(
                                &mut ctx.out_rels,
                                &class_key,
                                decorator,
                                source,
                                None,
                            );
                        }
                    }
                }
            }
        }
        "call_expression" => {
            if let (Some(source_key), Some(target_name)) = (scope, callee_name(node, source)) {
                ctx.pending.push(PendingRel {
                    source_key: source_key.to_string(),
                    kind: RelationshipKind::Calls,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        "new_expression" => {
            if let (Some(source_key), Some(target_name)) = (scope, constructor_name(node, source)) {
                ctx.pending.push(PendingRel {
                    source_key: source_key.to_string(),
                    kind: RelationshipKind::Constructs,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        // A type annotation (`: Foo`) on a variable/parameter/return type
        // that names a real declared type — not a primitive, union, or
        // other compound form (kept intentionally narrow). Falls through to
        // the shared recursion below; nothing further needs custom handling
        // inside a `type_annotation` node.
        "type_annotation" => {
            if let Some(target_name) = referenced_type_name(node, source) {
                ctx.pending.push(PendingRel {
                    source_key: reference_scope(parent, scope, relative_path),
                    kind: RelationshipKind::References,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect(child, parent, scope, ctx);
    }
}

/// Resolves `Inherits`/`Implements` (extends/implements clauses) once the
/// class's own key is known; targets aren't known as symbols yet (forward
/// declarations, other files), so this is deferred to `resolve_pending`.
fn collect_heritage(
    node: Node,
    source: &[u8],
    relative_path: &str,
    class_name: &str,
    pending: &mut Vec<PendingRel>,
) {
    let Some(heritage) = find_child(node, "class_heritage") else {
        return;
    };
    let class_key = qualified_name(relative_path, None, class_name);

    if let Some(extends) = find_child(heritage, "extends_clause") {
        if let Some(value) = extends.child_by_field_name("value") {
            if let Ok(target_name) = value.utf8_text(source) {
                pending.push(PendingRel {
                    source_key: class_key.clone(),
                    kind: RelationshipKind::Inherits,
                    target_name: target_name.to_string(),
                    span: span_of(extends),
                });
            }
        }
    }

    if let Some(implements) = find_child(heritage, "implements_clause") {
        let mut cursor = implements.walk();
        for ty in implements.named_children(&mut cursor) {
            if let Some(target_name) = type_name(ty, source) {
                pending.push(PendingRel {
                    source_key: class_key.clone(),
                    kind: RelationshipKind::Implements,
                    target_name,
                    span: span_of(ty),
                });
            }
        }
    }
}

/// Emits one `Imports` edge per imported binding (default/named/namespace)
/// in an `import_statement`. Module resolution (specifier → concrete file)
/// is a later phase slice — the target always carries the raw module
/// specifier text. `reason` carries the imported name (`"default"` for a
/// default import, `"*"` for a namespace import) so a later pass can look it
/// up in the target module's exports without re-parsing source.
fn collect_imports(
    node: Node,
    source: &[u8],
    relative_path: &str,
    out: &mut Vec<ExtractedRelationship>,
) {
    let Some(specifier) = module_specifier(node, source) else {
        return;
    };
    let Some(clause) = find_child(node, "import_clause") else {
        return;
    };
    let source_key = relative_path.to_string();

    let mut cursor = clause.walk();
    for part in clause.named_children(&mut cursor) {
        match part.kind() {
            "identifier" => push_unresolved_edge(
                out,
                &source_key,
                RelationshipKind::Imports,
                &specifier,
                span_of(part),
                Some("default".to_string()),
            ),
            "namespace_import" => push_unresolved_edge(
                out,
                &source_key,
                RelationshipKind::Imports,
                &specifier,
                span_of(part),
                Some("*".to_string()),
            ),
            "named_imports" => {
                let mut ic = part.walk();
                for spec in part.named_children(&mut ic) {
                    let Some(name_node) = spec.child_by_field_name("name") else {
                        continue;
                    };
                    let Ok(name) = name_node.utf8_text(source) else {
                        continue;
                    };
                    push_unresolved_edge(
                        out,
                        &source_key,
                        RelationshipKind::Imports,
                        &specifier,
                        span_of(spec),
                        Some(name.to_string()),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Emits `Exports` edges from an `export_statement`. Re-exports (a `from`
/// clause, or `export * from`) target `EdgeTarget::Unresolved(module_specifier)`
/// since the source module isn't known until whole-project resolution.
/// Local named/default exports defer through `pending` so they resolve
/// against this file's own symbol list, same as heritage edges. Direct
/// declaration exports (`export class Foo {}`) are intentionally not covered
/// — `is_exported` wiring is a later phase slice (see `push_named`'s
/// comment); their symbols still extract normally via the shared recursion
/// below.
fn collect_exports(
    node: Node,
    source: &[u8],
    relative_path: &str,
    out: &mut Vec<ExtractedRelationship>,
    pending: &mut Vec<PendingRel>,
) {
    let source_key = relative_path.to_string();
    let specifier = module_specifier(node, source);

    if let Some(clause) = find_child(node, "export_clause") {
        let mut cursor = clause.walk();
        for spec in clause.named_children(&mut cursor) {
            if spec.kind() != "export_specifier" {
                continue;
            }
            let Some(name_node) = spec.child_by_field_name("name") else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(source) else {
                continue;
            };
            match &specifier {
                Some(module) => push_unresolved_edge(
                    out,
                    &source_key,
                    RelationshipKind::Exports,
                    module,
                    span_of(spec),
                    Some(name.to_string()),
                ),
                None => pending.push(PendingRel {
                    source_key: source_key.clone(),
                    kind: RelationshipKind::Exports,
                    target_name: name.to_string(),
                    span: span_of(spec),
                }),
            }
        }
        return;
    }

    if let Some(module) = &specifier {
        // `export * from './mod'` (and `export * as ns from './mod'` — the
        // `ns` alias is dropped, this is extraction not resolution).
        push_unresolved_edge(
            out,
            &source_key,
            RelationshipKind::Exports,
            module,
            span_of(node),
            None,
        );
        return;
    }

    if is_default_export(node, source) {
        match export_default_name(node, source) {
            Some(name) => pending.push(PendingRel {
                source_key,
                kind: RelationshipKind::Exports,
                target_name: name,
                span: span_of(node),
            }),
            None => push_unresolved_edge(
                out,
                &source_key,
                RelationshipKind::Exports,
                "default",
                span_of(node),
                Some("anonymous default export".to_string()),
            ),
        }
    }
}

/// The module specifier text from an `import_statement`/`export_statement`'s
/// `source` string field, with surrounding quotes stripped.
fn module_specifier(node: Node, source: &[u8]) -> Option<String> {
    let string_node = node.child_by_field_name("source")?;
    let text = string_node.utf8_text(source).ok()?;
    Some(text.trim_matches(['\'', '"', '`']).to_string())
}

/// Whether an `export_statement` node is an `export default ...` (no
/// dedicated field distinguishes it in this grammar — the `default` keyword
/// is an anonymous token).
fn is_default_export(node: Node, source: &[u8]) -> bool {
    node.utf8_text(source)
        .map(|t| t.trim_start().starts_with("export default"))
        .unwrap_or(false)
}

/// The exported name for `export default X`, when determinable: a named
/// class/function declaration's own name, or a bare identifier value
/// (`export default someIdentifier;`). `None` for anonymous
/// declarations/expressions (`export default class {}`, `export default 42`).
fn export_default_name(node: Node, source: &[u8]) -> Option<String> {
    if let Some(decl) = node.child_by_field_name("declaration") {
        return decl
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::to_string);
    }
    let value = node.child_by_field_name("value")?;
    if value.kind() == "identifier" {
        return value.utf8_text(source).ok().map(str::to_string);
    }
    None
}

/// The callee name of a `call_expression`: the bare identifier for
/// `foo()`, or the accessed property for `this.foo()`/`obj.foo()`.
fn callee_name(node: Node, source: &[u8]) -> Option<String> {
    let func = node.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => func.utf8_text(source).ok().map(str::to_string),
        "member_expression" => func
            .child_by_field_name("property")
            .and_then(|p| p.utf8_text(source).ok())
            .map(str::to_string),
        _ => None,
    }
}

/// The constructed type name of a `new_expression` (`new Foo()`).
fn constructor_name(node: Node, source: &[u8]) -> Option<String> {
    let ctor = node.child_by_field_name("constructor")?;
    match ctor.kind() {
        "identifier" => ctor.utf8_text(source).ok().map(str::to_string),
        _ => None,
    }
}

/// A `type_identifier` or `generic_type` (e.g. `Comparable<T>`) name from an
/// `implements_clause` entry.
fn type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "generic_type" => node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::to_string),
        _ => node.utf8_text(source).ok().map(str::to_string),
    }
}

/// The plain named-type target of a `type_annotation` (`: Foo`), when it's a
/// simple type reference. Deliberately narrow — primitives (`string`,
/// `number`, ...), unions, and generics (`Array<Foo>`) are skipped so
/// `References` edges only ever target a real declared type, not TypeScript
/// built-ins or compound type expressions.
fn referenced_type_name(node: Node, source: &[u8]) -> Option<String> {
    let ty = node.named_child(0)?;
    if ty.kind() == "type_identifier" {
        return ty.utf8_text(source).ok().map(str::to_string);
    }
    None
}

/// The literal decorator name text (task 2.2 — no framework name is ever
/// special-cased here, only grammar shapes): `@Foo` → `Foo`; `@Foo(...)` →
/// `Foo`; `@ns.Foo` / `@ns.Foo(...)` → `Foo` (last segment, mirroring
/// `cs_simple_type_name`'s precedent); `@(expr)` unwraps one level of
/// parens. Anything else falls back to its own source text.
fn decorator_name(node: Node, source: &[u8]) -> Option<String> {
    let inner = node.named_child(0)?;
    decorator_expr_name(inner, source)
}

fn decorator_expr_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(source).ok().map(str::to_string),
        "call_expression" => {
            let func = node.child_by_field_name("function")?;
            decorator_expr_name(func, source)
        }
        "member_expression" => node
            .child_by_field_name("property")
            .and_then(|p| p.utf8_text(source).ok())
            .map(str::to_string),
        "parenthesized_expression" => {
            let inner = node.named_child(0)?;
            decorator_expr_name(inner, source)
        }
        _ => node.utf8_text(source).ok().map(str::to_string),
    }
}

/// Emits one `Decorates` edge for `decorator`, target
/// `Unresolved(<literal name>)`, span = the decorator's own span (task 2.2).
/// Silently skips a decorator whose name text can't be extracted rather than
/// guessing.
fn push_decorator_edge(
    out: &mut Vec<ExtractedRelationship>,
    source_key: &str,
    decorator: Node,
    source: &[u8],
    reason: Option<String>,
) {
    let Some(name) = decorator_name(decorator, source) else {
        return;
    };
    push_unresolved_edge(
        out,
        source_key,
        RelationshipKind::Decorates,
        &name,
        span_of(decorator),
        reason,
    );
}

/// The plain `name` field text of a class member node — used ahead of
/// `push_named` (by `class_body`'s decorator-pairing pass) so a member's
/// qualified key can be computed before the member itself is visited.
fn member_name_text(node: Node, source: &[u8]) -> Option<String> {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        .map(str::to_string)
}

/// Walks a constructor's `formal_parameters`, emitting one `Decorates` edge
/// per parameter decorator (task 2.3) — `reason = "param:<index>"`, 0-based
/// position in the parameter list, source = the constructor itself (D15: TS
/// synthesizes no parameter symbols).
fn collect_constructor_param_decorators(
    node: Node,
    method_key: &str,
    source: &[u8],
    out_rels: &mut Vec<ExtractedRelationship>,
) {
    let Some(params) = node.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for (index, param) in params.named_children(&mut cursor).enumerate() {
        if !matches!(param.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }
        let mut dcursor = param.walk();
        for decorator in param.children_by_field_name("decorator", &mut dcursor) {
            push_decorator_edge(
                out_rels,
                method_key,
                decorator,
                source,
                Some(format!("param:{index}")),
            );
        }
    }
}

/// The best-effort enclosing symbol for a `References` edge: the innermost
/// function/method scope (parameter/return type), else the enclosing class
/// (a field's type annotation), else the file itself (a top-level
/// variable's type annotation).
fn reference_scope(parent: Option<&str>, scope: Option<&str>, relative_path: &str) -> String {
    scope
        .map(str::to_string)
        .or_else(|| parent.map(|p| qualified_name(relative_path, None, p)))
        .unwrap_or_else(|| relative_path.to_string())
}

/// A function is top-level when it sits directly under the module, optionally
/// wrapped in an `export` statement.
fn is_top_level(node: Node) -> bool {
    match node.parent() {
        None => false,
        Some(parent) if parent.kind() == "program" => true,
        Some(parent) if parent.kind() == "export_statement" => {
            parent.parent().is_some_and(|g| g.kind() == "program")
        }
        Some(_) => false,
    }
}

fn method_kind(node: Node, source: &[u8]) -> SymbolKind {
    let is_constructor = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        .is_some_and(|name| name == "constructor");
    if is_constructor {
        SymbolKind::Constructor
    } else {
        SymbolKind::Method
    }
}

fn qualified_name(relative_path: &str, parent: Option<&str>, name: &str) -> String {
    match parent {
        Some(p) => format!("{relative_path}::{p}.{name}"),
        None => format!("{relative_path}::{name}"),
    }
}

/// Extracts `node`'s name and pushes an `ExtractedSymbol`, returning the name
/// so callers can use it as the `parent` for a nested scope (e.g. a class
/// body). Returns `None` (and pushes nothing) when the node has no name
/// field.
fn push_named(
    node: Node,
    source: &[u8],
    language: LanguageId,
    relative_path: &str,
    kind: SymbolKind,
    parent: Option<&str>,
    out: &mut Vec<ExtractedSymbol>,
) -> Option<String> {
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(source).ok()?.to_string();
    let qualified = qualified_name(relative_path, parent, &name);
    out.push(ExtractedSymbol {
        // ponytail: local_key = qualified_name for PR1 (single-file scope,
        // no cross-file collisions yet); revisit if pass 2 needs a distinct
        // pre-resolution key.
        local_key: qualified.clone(),
        name: name.clone(),
        qualified_name: qualified,
        kind,
        language,
        span: span_of(node),
        parent: parent.map(str::to_string),
        // ponytail: export detection deferred to import/export edge
        // extraction (whole-project resolution, later phase slice).
        is_exported: false,
        signature_fingerprint: crate::extract::fingerprint_fields(
            node,
            source,
            &["type_parameters", "parameters", "return_type"],
        ),
        // Phase 5 PR1: TypeScript has none of these concepts; set
        // explicitly (never via `Default::default()`) so the wiring is
        // visible at every construction site.
        visibility: codekurve_core::Visibility::Default,
        is_partial: false,
        is_record: false,
        partial_ordinal: None,
        // Phase 7: recognition (frameworks::recognize) sets roles after
        // analyze() returns; every analyzer construction site starts empty.
        roles: Vec::new(),
    });
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ExtractedRelationship;

    const SOURCE: &str = r#"
export class MemberService {
  find(id: string) { return id; }
}

export function topLevel(): void {}

function alsoTop() {
  function nested() {}
  return nested;
}
"#;

    #[test]
    fn extracts_classes_functions_and_methods() {
        let analysis = analyze(SOURCE, LanguageId::TypeScript, "src/member.ts").unwrap();
        let names: Vec<(&str, SymbolKind)> = analysis
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();

        assert!(names.contains(&("MemberService", SymbolKind::Class)));
        assert!(names.contains(&("topLevel", SymbolKind::Function)));
        assert!(names.contains(&("alsoTop", SymbolKind::Function)));
        assert!(names.contains(&("find", SymbolKind::Method)));
        // Nested function inside a function body is not extracted.
        assert!(!names.iter().any(|(n, _)| *n == "nested"));
    }

    #[test]
    fn span_points_at_declaration() {
        let analysis = analyze(SOURCE, LanguageId::TypeScript, "src/member.ts").unwrap();
        let svc = analysis
            .symbols
            .iter()
            .find(|s| s.name == "MemberService")
            .unwrap();
        assert_eq!(svc.span.start_line, 2);
        assert!(svc.span.end_byte > svc.span.start_byte);
    }

    /// Spec scenario "Nested member qualified name" (Requirement "Real
    /// Qualified Name Computation").
    #[test]
    fn nested_method_qualified_name() {
        let source =
            "export class MemberService {\n  getEligibility(id: string) { return id; }\n}\n";
        let analysis = analyze(
            source,
            LanguageId::TypeScript,
            "src/services/member.service.ts",
        )
        .unwrap();

        let method = analysis
            .symbols
            .iter()
            .find(|s| s.name == "getEligibility")
            .unwrap();
        assert_eq!(
            method.qualified_name,
            "src/services/member.service.ts::MemberService.getEligibility"
        );
        assert_eq!(method.parent.as_deref(), Some("MemberService"));
    }

    /// Spec scenario "Contains hierarchy" (Requirement "Relationship Kind
    /// Extraction").
    #[test]
    fn contains_edges_link_class_to_methods() {
        let source = "class Box {\n  a() {}\n  b() {}\n}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/box.ts").unwrap();

        let contains: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Contains)
            .collect();
        assert_eq!(contains.len(), 2);
        for edge in &contains {
            assert_eq!(edge.source_local_key, "src/box.ts::Box");
            assert_eq!(edge.provenance, Provenance::Extracted);
            assert_eq!(edge.confidence, Confidence::Exact);
        }
    }

    /// Spec scenario "Class extends and implements" (Requirement
    /// "Relationship Kind Extraction").
    #[test]
    fn heritage_edges_extends_and_implements() {
        let source = "class Base {}\nclass Foo extends Base implements IFoo {}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/heritage.ts").unwrap();

        let extends = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Inherits)
            .unwrap();
        assert_eq!(extends.source_local_key, "src/heritage.ts::Foo");
        assert_eq!(
            extends.target,
            EdgeTarget::Local("src/heritage.ts::Base".to_string())
        );
        assert_eq!(extends.confidence, Confidence::Exact);

        // No same-file symbol named IFoo (interfaces aren't extracted as
        // symbols yet) — the edge still exists, per §18.3, rather than being
        // silently dropped.
        let implements = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Implements)
            .unwrap();
        assert_eq!(implements.source_local_key, "src/heritage.ts::Foo");
        assert_eq!(
            implements.target,
            EdgeTarget::Unresolved("IFoo".to_string())
        );
        assert_eq!(implements.confidence, Confidence::Unresolved);
    }

    /// A method calling a sibling method resolves to a same-file `Calls`
    /// edge (spec "Exact local call").
    #[test]
    fn method_call_to_sibling_resolves_locally() {
        let source =
            "class Service {\n  run() { return this.helper(); }\n  helper() { return 1; }\n}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/service.ts").unwrap();

        let call = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Calls)
            .unwrap();
        assert_eq!(call.source_local_key, "src/service.ts::Service.run");
        assert_eq!(
            call.target,
            EdgeTarget::Local("src/service.ts::Service.helper".to_string())
        );
        assert_eq!(call.confidence, Confidence::Exact);
    }

    /// A `new` expression targeting a same-file class resolves to a
    /// `Constructs` edge (spec "Exact local call" tier applies equally to
    /// constructs).
    #[test]
    fn new_expression_resolves_to_local_class() {
        let source = "class Widget {}\nfunction build() { return new Widget(); }\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/widget.ts").unwrap();

        let construct = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Constructs)
            .unwrap();
        assert_eq!(construct.source_local_key, "src/widget.ts::build");
        assert_eq!(
            construct.target,
            EdgeTarget::Local("src/widget.ts::Widget".to_string())
        );
        assert_eq!(construct.confidence, Confidence::Exact);
    }

    /// An `interface_declaration` is extracted as a `SymbolKind::Interface`
    /// symbol, and a same-file `implements` clause now resolves to it
    /// (spec "Class extends and implements" — the in-project case).
    #[test]
    fn interface_is_extracted_and_implements_resolves_locally() {
        let source = "interface IFoo {}\nclass Foo implements IFoo {}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/iface.ts").unwrap();

        let iface = analysis
            .symbols
            .iter()
            .find(|s| s.name == "IFoo")
            .expect("IFoo extracted as a symbol");
        assert_eq!(iface.kind, SymbolKind::Interface);

        let implements = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Implements)
            .unwrap();
        assert_eq!(
            implements.target,
            EdgeTarget::Local("src/iface.ts::IFoo".to_string())
        );
        assert_eq!(implements.confidence, Confidence::Exact);
    }

    /// A parameter/return type annotation naming a real declared type emits
    /// a `References` edge (spec "Relationship Kind Extraction" — `Foo` is
    /// one of the MUST-extracted kinds); a primitive type (`string`) does
    /// not.
    #[test]
    fn type_annotation_emits_references_edge() {
        let source =
            "class Widget {}\nfunction build(): Widget { return new Widget(); }\nfunction id(x: string): string { return x; }\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/refs.ts").unwrap();

        let references: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::References)
            .collect();
        assert_eq!(references.len(), 1, "only Widget is a real type reference");
        let reference = references[0];
        assert_eq!(reference.source_local_key, "src/refs.ts::build");
        assert_eq!(
            reference.target,
            EdgeTarget::Local("src/refs.ts::Widget".to_string())
        );
        assert_eq!(reference.confidence, Confidence::Exact);
    }

    /// design "symbol_key and signature_fingerprint": params/return-type
    /// text feeds the fingerprint (whitespace-normalized); a class has no
    /// call signature, so its fingerprint is empty.
    #[test]
    fn signature_fingerprint_reflects_params_and_return_type() {
        let source = "function id(x: string): string { return x; }\nclass Empty {}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/sig.ts").unwrap();

        let func = analysis.symbols.iter().find(|s| s.name == "id").unwrap();
        assert_eq!(func.signature_fingerprint, "(x: string)\u{1f}: string");

        let class = analysis.symbols.iter().find(|s| s.name == "Empty").unwrap();
        assert_eq!(class.signature_fingerprint, "");
    }

    /// Extra internal whitespace within the parameter list (that doesn't
    /// shift what touches the surrounding parens/brackets) collapses to the
    /// same fingerprint — `split_whitespace().join(" ")` normalizes runs of
    /// whitespace, not token adjacency.
    #[test]
    fn signature_fingerprint_ignores_extra_internal_whitespace() {
        let a = analyze(
            "function f(x: string): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        let b = analyze(
            "function f(x:    string): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        assert_eq!(
            a.symbols[0].signature_fingerprint,
            b.symbols[0].signature_fingerprint
        );
    }

    /// Task 2.11: the same TypeScript source analyzed twice through the
    /// post-seam dispatch path (`crate::extract::analyze` ->
    /// `analyzer_for` -> `TypeScriptAnalyzer::analyze`) produces an
    /// identical `FileAnalysis` — symbols, relationships, and unresolved
    /// references all compare equal, proving the move introduced no
    /// nondeterminism.
    #[test]
    fn same_source_produces_identical_file_analysis() {
        let source = "class Base {}\nclass Foo extends Base implements IFoo {\n  run() { return this.helper(); }\n  helper() { return new Base(); }\n}\n";
        let first =
            crate::extract::analyze(source, LanguageId::TypeScript, "src/idempotent.ts").unwrap();
        let second =
            crate::extract::analyze(source, LanguageId::TypeScript, "src/idempotent.ts").unwrap();
        assert_eq!(first, second);
    }

    /// A genuine signature change (parameter added) must change the
    /// fingerprint (spec "Rename changes identity" applies equally to
    /// signature edits feeding `symbol_key`).
    #[test]
    fn signature_fingerprint_changes_when_parameters_differ() {
        let a = analyze(
            "function f(x: string): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        let b = analyze(
            "function f(x: string, y: number): void {}\n",
            LanguageId::TypeScript,
            "src/sig.ts",
        )
        .unwrap();
        assert_ne!(
            a.symbols[0].signature_fingerprint,
            b.symbols[0].signature_fingerprint
        );
    }

    /// Task 2.5: class, method, property, and constructor-parameter
    /// decorators each produce exactly one `Decorates` edge carrying the
    /// literal decorator name text and the decorator's own span (spec
    /// "TypeScript class decorator produces a decorates edge").
    #[test]
    fn all_four_decorator_positions_produce_decorates_edges() {
        let source = r#"
@Component({})
export class InvoiceList {
  @Input() name: string;

  @HostListener('click')
  onClick() {}

  constructor(@Inject(TOKEN) private svc: Foo) {}
}
"#;
        let analysis = analyze(source, LanguageId::TypeScript, "src/invoice-list.ts").unwrap();
        let decorates: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Decorates)
            .collect();
        assert_eq!(decorates.len(), 4, "class + property + method + param");

        let class_edge = decorates
            .iter()
            .find(|r| r.source_local_key == "src/invoice-list.ts::InvoiceList")
            .expect("class decorator");
        assert_eq!(
            class_edge.target,
            EdgeTarget::Unresolved("Component".to_string())
        );
        assert!(class_edge.span.start_byte < class_edge.span.end_byte);
        // The edge's own span is the decorator, not the whole class.
        assert!(source[class_edge.span.start_byte..class_edge.span.end_byte].starts_with('@'));

        let property_edge = decorates
            .iter()
            .find(|r| r.source_local_key == "src/invoice-list.ts::InvoiceList.name")
            .expect("property decorator");
        assert_eq!(
            property_edge.target,
            EdgeTarget::Unresolved("Input".to_string())
        );

        let method_edge = decorates
            .iter()
            .find(|r| r.source_local_key == "src/invoice-list.ts::InvoiceList.onClick")
            .expect("method decorator");
        assert_eq!(
            method_edge.target,
            EdgeTarget::Unresolved("HostListener".to_string())
        );

        let param_edge = decorates
            .iter()
            .find(|r| r.source_local_key == "src/invoice-list.ts::InvoiceList.constructor")
            .expect("constructor-param decorator");
        assert_eq!(
            param_edge.target,
            EdgeTarget::Unresolved("Inject".to_string())
        );
        assert_eq!(param_edge.reason.as_deref(), Some("param:0"));
    }

    /// Task 2.6: `@Inject(TOKEN) private svc: Foo` on a constructor
    /// parameter → `Decorates` edge with `reason = "param:<index>"` (spec
    /// "TypeScript constructor-parameter decorator produces a decorates
    /// edge").
    #[test]
    fn constructor_param_decorator_carries_param_index_reason() {
        let source =
            "class Widget {\n  constructor(plain: Bar, @Inject(TOKEN) private svc: Foo) {}\n}\n";
        let analysis = analyze(source, LanguageId::TypeScript, "src/widget-di.ts").unwrap();

        let param_edge = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Decorates)
            .expect("one Decorates edge for the decorated parameter");
        assert_eq!(
            param_edge.source_local_key,
            "src/widget-di.ts::Widget.constructor"
        );
        assert_eq!(
            param_edge.target,
            EdgeTarget::Unresolved("Inject".to_string())
        );
        // svc is the second parameter (index 1) — plain has no decorator and
        // emits nothing.
        assert_eq!(param_edge.reason.as_deref(), Some("param:1"));
    }

    /// Task 2.7: exhaustive `(RelationshipKind, SymbolKind)` sweep —
    /// `Decorates` matches unconditionally (mirroring `CSharpAnalyzer`'s own
    /// `_ => true`), and every non-`Decorates` pair is unchanged from the
    /// pre-existing table (spec "Existing TypeScript kind_matches answers
    /// are unaffected").
    #[test]
    fn kind_matches_accepts_decorates_and_leaves_other_answers_unchanged() {
        use crate::languages::analyzer_for;

        const ALL_SYMBOL_KINDS: [SymbolKind; 16] = [
            SymbolKind::Module,
            SymbolKind::Namespace,
            SymbolKind::Class,
            SymbolKind::Interface,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Function,
            SymbolKind::Method,
            SymbolKind::Constructor,
            SymbolKind::Property,
            SymbolKind::Field,
            SymbolKind::Variable,
            SymbolKind::Parameter,
            SymbolKind::TypeAlias,
            SymbolKind::Import,
            SymbolKind::Export,
        ];

        fn pre_pr2_kind_matches(rel_kind: RelationshipKind, sym_kind: SymbolKind) -> bool {
            match rel_kind {
                RelationshipKind::Constructs => sym_kind == SymbolKind::Class,
                RelationshipKind::Calls => matches!(
                    sym_kind,
                    SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
                ),
                RelationshipKind::Inherits | RelationshipKind::Implements => {
                    matches!(sym_kind, SymbolKind::Class | SymbolKind::Interface)
                }
                RelationshipKind::References => {
                    matches!(
                        sym_kind,
                        SymbolKind::Class | SymbolKind::Interface | SymbolKind::TypeAlias
                    )
                }
                RelationshipKind::Exports => {
                    matches!(
                        sym_kind,
                        SymbolKind::Class | SymbolKind::Function | SymbolKind::Interface
                    )
                }
                _ => true,
            }
        }

        for analyzer in [
            analyzer_for(LanguageId::TypeScript),
            analyzer_for(LanguageId::JavaScript),
        ] {
            for &sym_kind in &ALL_SYMBOL_KINDS {
                assert!(
                    analyzer.kind_matches(RelationshipKind::Decorates, sym_kind),
                    "Decorates must match every symbol kind, got false for {sym_kind:?}"
                );
                for &rel_kind in &[
                    RelationshipKind::Defines,
                    RelationshipKind::Contains,
                    RelationshipKind::Imports,
                    RelationshipKind::Exports,
                    RelationshipKind::References,
                    RelationshipKind::Calls,
                    RelationshipKind::Constructs,
                    RelationshipKind::Inherits,
                    RelationshipKind::Implements,
                    RelationshipKind::Overrides,
                    RelationshipKind::UsesType,
                    RelationshipKind::Reads,
                    RelationshipKind::Writes,
                ] {
                    assert_eq!(
                        analyzer.kind_matches(rel_kind, sym_kind),
                        pre_pr2_kind_matches(rel_kind, sym_kind),
                        "mismatch for ({rel_kind:?}, {sym_kind:?})"
                    );
                }
            }
        }
    }

    /// Task 2.8: no framework-specific string literal (Angular/ASP.NET/EF
    /// Core catalogue names) is ever compared/matched against inside the
    /// *production* extraction logic of `languages/typescript.rs` or
    /// `languages/csharp.rs` (spec "No Angular-specific ... decorator name
    /// MUST be special-cased inside `languages/typescript.rs`"), following
    /// `scripts/check_licensing.py`'s grep-based-check precedent. Comments
    /// and the `#[cfg(test)]` module are stripped first — both files
    /// legitimately use realistic decorator/attribute names (`@Component`,
    /// `[HttpGet]`, ...) as generic, framework-blind extraction examples;
    /// this check is about branching logic, not illustrative test fixtures.
    #[test]
    fn no_framework_specific_names_in_language_analyzer_logic() {
        const FRAMEWORK_MARKERS: &[&str] = &[
            "Component",
            "Injectable",
            "NgModule",
            "Directive",
            "HostListener",
            "Angular",
            "ApiController",
            "HttpGet",
            "HttpPost",
            "DbSet",
            "DbContext",
            "AddScoped",
            "AddSingleton",
            "AddTransient",
            "UseMiddleware",
            "MapGet",
            "MapPost",
            "AzureFunction",
            "TimerTrigger",
            "QueueTrigger",
        ];
        for (path, contents) in [
            ("src/languages/typescript.rs", include_str!("typescript.rs")),
            ("src/languages/csharp.rs", include_str!("csharp.rs")),
        ] {
            let production_code = production_code_only(contents);
            for marker in FRAMEWORK_MARKERS {
                assert!(
                    !production_code.contains(marker),
                    "{path} contains framework-specific marker {marker:?} outside comments/tests"
                );
            }
        }
    }

    /// Drops everything from `#[cfg(test)]` onward (the test module) and
    /// every `//`/`///` comment, leaving only the compiled, non-test source
    /// text that task 2.8 cares about.
    fn production_code_only(src: &str) -> String {
        let code = src.split("#[cfg(test)]").next().unwrap_or(src);
        code.lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Task 2.9: runtime harness — decorator extraction on a scratch `.ts`
    /// source covering all four decorator positions, output asserted by
    /// hand (name text + span + reason for every edge, no framework
    /// semantics inferred).
    #[test]
    fn runtime_harness_scratch_file_all_decorator_positions() {
        let source = r#"
@Injectable()
export class TokenStore {
  @Input() token: string;

  @HostListener('window:resize')
  onResize() {}

  constructor(@Inject(WINDOW) private win: Window, plain: Logger) {}
}
"#;
        let analysis = analyze(source, LanguageId::TypeScript, "src/token-store.ts").unwrap();
        let mut decorates: Vec<(&str, String, Option<&str>)> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Decorates)
            .map(|r| {
                let EdgeTarget::Unresolved(name) = &r.target else {
                    panic!("Decorates target must be Unresolved");
                };
                (
                    r.source_local_key.as_str(),
                    name.clone(),
                    r.reason.as_deref(),
                )
            })
            .collect();
        decorates.sort();

        let mut expected = vec![
            (
                "src/token-store.ts::TokenStore",
                "Injectable".to_string(),
                None,
            ),
            (
                "src/token-store.ts::TokenStore.token",
                "Input".to_string(),
                None,
            ),
            (
                "src/token-store.ts::TokenStore.onResize",
                "HostListener".to_string(),
                None,
            ),
            (
                "src/token-store.ts::TokenStore.constructor",
                "Inject".to_string(),
                Some("param:0"),
            ),
        ];
        expected.sort();

        assert_eq!(decorates, expected);
    }
}
