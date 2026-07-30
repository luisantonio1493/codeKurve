//! C# symbol + relationship extraction (design "C# Node-Kind Mapping",
//! "Visibility", "qualified_name", "Partial identity"; Phase 5 PR3 + PR4).
//! `CsCtx` is its own type, not shared with TypeScript's `CollectCtx` (design
//! "Module Layout"): it tracks a namespace stack and a type stack instead of
//! a single `parent` string, and its `push`-style helpers read
//! visibility/`partial`/`record` that TypeScript has no concept of.
//!
//! PR4 adds edge extraction: `using` directives (`Imports`), base-list
//! entries (`UsesType` + `BASE_LIST_REASON`, never through `resolve_pending`
//! — design "Architecture Decisions"), calls/object-creation (`Calls`/
//! `Constructs`, deferred via `PendingRel`), and attributes (`Decorates`,
//! never deferred either). No resolution/candidate-matching logic lives
//! here — that is PR5's job (design "Resolution Changes").

use std::collections::HashMap;

use codekurve_core::error::{Error, Result};
use codekurve_core::{
    Confidence, LanguageId, Provenance, RelationshipKind, SourceSpan, SymbolKind, Visibility,
};
use tree_sitter::{Node, Parser};

use crate::extract::{find_child, span_of};
use crate::ir::{EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis};
use crate::languages::{
    analyzer_for, push_unresolved_edge, resolve_pending, LanguageAnalyzer, PendingRel,
    BASE_LIST_REASON,
};

/// design "C# Node-Kind Mapping": `implicit_object_creation_expression` (the
/// target-typed `new()` form) has no type name at the call site — emitted as
/// an unresolved `Constructs` edge with this reason rather than dropped or
/// guessed.
const TARGET_TYPED_NEW_REASON: &str = "target-typed new() has no type name at the call site";

pub struct CSharpAnalyzer;

pub(crate) const CS: CSharpAnalyzer = CSharpAnalyzer;

impl LanguageAnalyzer for CSharpAnalyzer {
    fn language(&self) -> LanguageId {
        LanguageId::CSharp
    }

    fn analyze(&self, source: &str, relative_path: &str) -> Result<FileAnalysis> {
        analyze(source, relative_path)
    }

    fn kind_matches(&self, rel: RelationshipKind, sym: SymbolKind) -> bool {
        match rel {
            RelationshipKind::Constructs => matches!(sym, SymbolKind::Class | SymbolKind::Struct),
            RelationshipKind::Calls => matches!(
                sym,
                SymbolKind::Method | SymbolKind::Constructor | SymbolKind::Function
            ),
            RelationshipKind::Inherits => matches!(sym, SymbolKind::Class | SymbolKind::Struct),
            RelationshipKind::Implements => sym == SymbolKind::Interface,
            RelationshipKind::UsesType | RelationshipKind::References => matches!(
                sym,
                SymbolKind::Class | SymbolKind::Struct | SymbolKind::Interface | SymbolKind::Enum
            ),
            RelationshipKind::Imports => sym == SymbolKind::Namespace,
            RelationshipKind::Exports => false,
            _ => true,
        }
    }
}

/// Everything the recursive walk needs that doesn't change with tree depth.
/// `namespace_stack`/`type_stack` hold the *plain* names of enclosing
/// namespaces/types (not local keys) so `cs_qualified_name` can join them;
/// `partial_ordinals` is keyed by `qualified_name` so two `partial`
/// fragments of the same type/method in one file get distinct ordinals
/// (design "Partial identity").
struct CsCtx<'a> {
    source: &'a [u8],
    relative_path: &'a str,
    namespace_stack: Vec<String>,
    type_stack: Vec<String>,
    partial_ordinals: HashMap<String, u32>,
    out: Vec<ExtractedSymbol>,
    out_rels: Vec<ExtractedRelationship>,
    /// Deferred `Calls`/`Constructs` targets (design "C# Node-Kind Mapping"
    /// rows for `invocation_expression`/`object_creation_expression") —
    /// resolved same-file at the end of `analyze` via `resolve_pending`, same
    /// as TypeScript. `using`/base-list/attribute edges never go through this
    /// list.
    pending: Vec<PendingRel>,
}

fn analyze(source: &str, relative_path: &str) -> Result<FileAnalysis> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|e| Error::Parse(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| Error::Parse("parser returned no tree".to_string()))?;

    let mut ctx = CsCtx {
        source: source.as_bytes(),
        relative_path,
        namespace_stack: Vec::new(),
        type_stack: Vec::new(),
        partial_ordinals: HashMap::new(),
        out: Vec::new(),
        out_rels: Vec::new(),
        pending: Vec::new(),
    };

    // A `file_scoped_namespace_declaration` has no body — it applies to every
    // following top-level declaration in the file (design "C# Node-Kind
    // Mapping"), so the top-level walk is a manual loop that threads a
    // mutable "current container" across siblings, rather than the fixed
    // per-call `container_key` the rest of `collect` uses.
    let root = tree.root_node();
    let mut container: Option<String> = None;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "file_scoped_namespace_declaration" {
            if let Some(key) = handle_file_scoped_namespace(child, container.as_deref(), &mut ctx) {
                container = Some(key);
            }
            continue;
        }
        collect(child, container.as_deref(), &mut ctx);
    }

    let mut relationships = ctx.out_rels;
    resolve_pending(
        &ctx.out,
        ctx.pending,
        &mut relationships,
        analyzer_for(LanguageId::CSharp),
    );

    Ok(FileAnalysis {
        file: relative_path.to_string(),
        language: LanguageId::CSharp,
        symbols: ctx.out,
        relationships,
        unresolved: Vec::new(),
        diagnostics: Vec::new(),
    })
}

/// Recurses over one node, dispatching to a per-declaration handler and
/// falling through to plain recursion for everything else (statements,
/// expressions — nothing there is a PR3 symbol). `container_key` is the
/// local key of the nearest enclosing namespace/type/enum, used as the
/// `Contains` source when this node turns out to be a declaration.
fn collect(node: Node, container_key: Option<&str>, ctx: &mut CsCtx) {
    match node.kind() {
        "namespace_declaration" => {
            handle_block_namespace(node, container_key, ctx);
        }
        "class_declaration"
        | "interface_declaration"
        | "struct_declaration"
        | "record_declaration" => {
            handle_type_decl(node, container_key, ctx);
        }
        "enum_declaration" => {
            handle_enum_decl(node, container_key, ctx);
        }
        "constructor_declaration" | "method_declaration" | "property_declaration" => {
            handle_member(node, container_key, ctx);
        }
        "field_declaration" => {
            handle_field_decl(node, container_key, ctx);
        }
        "using_directive" => {
            collect_using(node, ctx);
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect(child, container_key, ctx);
            }
        }
    }
}

/// Block-scoped `namespace X { ... }`: pushes/pops the namespace stack
/// around its body so sibling declarations after the closing brace don't see
/// it (unlike the file-scoped form).
fn handle_block_namespace(node: Node, container_key: Option<&str>, ctx: &mut CsCtx) {
    let Some((local_key, full_name)) = push_namespace_symbol(node, container_key, ctx) else {
        return;
    };
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            collect(child, Some(local_key.as_str()), ctx);
        }
    }
    debug_assert_eq!(ctx.namespace_stack.last(), Some(&full_name));
    ctx.namespace_stack.pop();
}

/// `namespace X;` at the top of a file — no body, applies to every following
/// top-level declaration. Returns the new container key (the caller keeps it
/// active across subsequent siblings) without popping the namespace stack.
fn handle_file_scoped_namespace(
    node: Node,
    container_key: Option<&str>,
    ctx: &mut CsCtx,
) -> Option<String> {
    push_namespace_symbol(node, container_key, ctx).map(|(local_key, _)| local_key)
}

/// Shared by both namespace forms: extracts the name, computes the fully
/// dotted name (design "Namespace symbol name" — dotted even across lexical
/// nesting), pushes the symbol and the `Contains` edge, and pushes the new
/// segment onto `namespace_stack`. Returns `(local_key, full_name)` so the
/// caller can pop the right thing.
fn push_namespace_symbol(
    node: Node,
    container_key: Option<&str>,
    ctx: &mut CsCtx,
) -> Option<(String, String)> {
    let name_node = node.child_by_field_name("name")?;
    let raw_name = name_node.utf8_text(ctx.source).ok()?;
    let full_name = if ctx.namespace_stack.is_empty() {
        raw_name.to_string()
    } else {
        format!("{}.{}", ctx.namespace_stack.join("."), raw_name)
    };
    let qualified = cs_qualified_name(ctx.relative_path, &[], &[], &full_name);
    let local_key = qualified.clone();
    let parent = if ctx.namespace_stack.is_empty() {
        None
    } else {
        Some(ctx.namespace_stack.join("."))
    };

    if let Some(container) = container_key {
        push_contains(&mut ctx.out_rels, container, &local_key, span_of(node));
    }
    ctx.out.push(ExtractedSymbol {
        local_key: local_key.clone(),
        name: full_name.clone(),
        qualified_name: qualified,
        kind: SymbolKind::Namespace,
        language: LanguageId::CSharp,
        span: span_of(node),
        parent,
        is_exported: false,
        signature_fingerprint: String::new(),
        visibility: Visibility::Default,
        is_partial: false,
        is_record: false,
        partial_ordinal: None,
    });
    ctx.namespace_stack.push(full_name.clone());
    Some((local_key, full_name))
}

/// `class_declaration`/`interface_declaration`/`struct_declaration`/
/// `record_declaration` → `Class`/`Interface`/`Struct` (+ `is_record` for
/// the latter), visibility, `partial`, nested-type recursion.
fn handle_type_decl(node: Node, container_key: Option<&str>, ctx: &mut CsCtx) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(ctx.source) else {
        return;
    };
    let (kind, is_record) = match node.kind() {
        "class_declaration" => (SymbolKind::Class, false),
        "interface_declaration" => (SymbolKind::Interface, false),
        "struct_declaration" => (SymbolKind::Struct, false),
        // `record class Foo`/`record Foo` parse as `record_declaration` with
        // no distinguishing keyword child; `record struct Foo` has an
        // unnamed `struct` token among its children (design Open Questions,
        // resolved against the pinned grammar's actual parse tree).
        "record_declaration" if has_child_kind(node, "struct") => (SymbolKind::Struct, true),
        "record_declaration" => (SymbolKind::Class, true),
        _ => unreachable!("collect only dispatches type-declaration kinds here"),
    };

    let is_partial = has_modifier(node, ctx.source, "partial");
    let visibility = visibility_of(node, ctx.source);
    let qualified = cs_qualified_name(
        ctx.relative_path,
        &ctx.namespace_stack,
        &ctx.type_stack,
        name,
    );
    let partial_ordinal = is_partial.then(|| next_partial_ordinal(ctx, &qualified));
    let signature_fingerprint = cs_fingerprint(node, ctx.source);
    let local_key = local_key_for(&qualified, partial_ordinal, &signature_fingerprint);
    let parent = enclosing_name(ctx);

    if let Some(container) = container_key {
        push_contains(&mut ctx.out_rels, container, &local_key, span_of(node));
    }
    ctx.out.push(ExtractedSymbol {
        local_key: local_key.clone(),
        name: name.to_string(),
        qualified_name: qualified,
        kind,
        language: LanguageId::CSharp,
        span: span_of(node),
        parent,
        is_exported: false,
        signature_fingerprint,
        visibility,
        is_partial,
        is_record,
        partial_ordinal,
    });
    collect_bases(node, &local_key, ctx);
    collect_attributes(node, &local_key, ctx);

    ctx.type_stack.push(name.to_string());
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.named_children(&mut cursor) {
            collect(child, Some(local_key.as_str()), ctx);
        }
    }
    ctx.type_stack.pop();
}

/// `enum_declaration` → `Enum`; each `enum_member_declaration` → `Field`
/// with the enum as parent (design "Enum members", spec "Enum members index
/// as Field"). No `SymbolKind::EnumMember` — finalized decision.
fn handle_enum_decl(node: Node, container_key: Option<&str>, ctx: &mut CsCtx) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(ctx.source) else {
        return;
    };
    let visibility = visibility_of(node, ctx.source);
    let qualified = cs_qualified_name(
        ctx.relative_path,
        &ctx.namespace_stack,
        &ctx.type_stack,
        name,
    );
    let local_key = qualified.clone();
    let parent = enclosing_name(ctx);

    if let Some(container) = container_key {
        push_contains(&mut ctx.out_rels, container, &local_key, span_of(node));
    }
    ctx.out.push(ExtractedSymbol {
        local_key: local_key.clone(),
        name: name.to_string(),
        qualified_name: qualified,
        kind: SymbolKind::Enum,
        language: LanguageId::CSharp,
        span: span_of(node),
        parent,
        is_exported: false,
        signature_fingerprint: String::new(),
        visibility,
        is_partial: false,
        is_record: false,
        partial_ordinal: None,
    });
    collect_attributes(node, &local_key, ctx);

    let Some(body) = node.child_by_field_name("body") else {
        return;
    };
    let mut member_types = ctx.type_stack.clone();
    member_types.push(name.to_string());
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if member.kind() != "enum_member_declaration" {
            continue;
        }
        let Some(mname_node) = member.child_by_field_name("name") else {
            continue;
        };
        let Ok(mname) = mname_node.utf8_text(ctx.source) else {
            continue;
        };
        let mqualified = cs_qualified_name(
            ctx.relative_path,
            &ctx.namespace_stack,
            &member_types,
            mname,
        );
        let member_local_key = mqualified.clone();
        push_contains(
            &mut ctx.out_rels,
            &local_key,
            &member_local_key,
            span_of(member),
        );
        ctx.out.push(ExtractedSymbol {
            local_key: member_local_key.clone(),
            name: mname.to_string(),
            qualified_name: mqualified,
            kind: SymbolKind::Field,
            language: LanguageId::CSharp,
            span: span_of(member),
            parent: Some(name.to_string()),
            is_exported: false,
            signature_fingerprint: String::new(),
            visibility: Visibility::Default,
            is_partial: false,
            is_record: false,
            partial_ordinal: None,
        });
        collect_attributes(member, &member_local_key, ctx);
    }
}

/// `constructor_declaration`/`method_declaration`/`property_declaration` →
/// `Constructor`/`Method`/`Property`. No PR3 symbol lives inside a body, but
/// PR4 walks it (with this member's own key as `scope`) for `Calls`/
/// `Constructs` attribution (design "C# Node-Kind Mapping").
fn handle_member(node: Node, container_key: Option<&str>, ctx: &mut CsCtx) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(ctx.source) else {
        return;
    };
    let kind = match node.kind() {
        "constructor_declaration" => SymbolKind::Constructor,
        "method_declaration" => SymbolKind::Method,
        "property_declaration" => SymbolKind::Property,
        _ => unreachable!("collect only dispatches member kinds here"),
    };
    // `partial` only applies to methods (partial classes' own `is_partial`
    // is handled in `handle_type_decl`); constructors/properties can't carry
    // it.
    let is_partial =
        node.kind() == "method_declaration" && has_modifier(node, ctx.source, "partial");
    let visibility = visibility_of(node, ctx.source);
    let qualified = cs_qualified_name(
        ctx.relative_path,
        &ctx.namespace_stack,
        &ctx.type_stack,
        name,
    );
    let partial_ordinal = is_partial.then(|| next_partial_ordinal(ctx, &qualified));
    let signature_fingerprint = cs_fingerprint(node, ctx.source);
    let local_key = local_key_for(&qualified, partial_ordinal, &signature_fingerprint);
    let parent = ctx.type_stack.last().cloned();

    if let Some(container) = container_key {
        push_contains(&mut ctx.out_rels, container, &local_key, span_of(node));
    }
    ctx.out.push(ExtractedSymbol {
        local_key: local_key.clone(),
        name: name.to_string(),
        qualified_name: qualified,
        kind,
        language: LanguageId::CSharp,
        span: span_of(node),
        parent,
        is_exported: false,
        signature_fingerprint,
        visibility,
        is_partial,
        is_record: false,
        partial_ordinal,
    });
    collect_attributes(node, &local_key, ctx);
    collect_member_body(node, &local_key, ctx);
}

/// Walks a method/constructor/property's executable body (design "C#
/// Node-Kind Mapping": "body walked with `scope = ctor|method key`",
/// "accessor bodies walked with `scope = property key`"). Properties can
/// carry an `accessors` accessor-list (`get`/`set`/`init`) and/or a direct
/// `value` (expression-bodied `=> ...`); methods/constructors carry a single
/// `body` (block or arrow expression).
fn collect_member_body(node: Node, scope: &str, ctx: &mut CsCtx) {
    if let Some(body) = node.child_by_field_name("body") {
        walk_expressions(body, scope, ctx);
    }
    if let Some(value) = node.child_by_field_name("value") {
        walk_expressions(value, scope, ctx);
    }
    if let Some(accessors) = node.child_by_field_name("accessors") {
        let mut cursor = accessors.walk();
        for accessor in accessors.named_children(&mut cursor) {
            if let Some(body) = accessor.child_by_field_name("body") {
                walk_expressions(body, scope, ctx);
            }
        }
    }
}

/// Recursively finds `invocation_expression`/`object_creation_expression`/
/// `implicit_object_creation_expression` nodes within a body and either
/// defers them via `PendingRel` (calls/constructs with a real type name) or
/// emits an unresolved `Constructs` edge directly (target-typed `new()` —
/// design "C# Node-Kind Mapping", never dropped or guessed).
fn walk_expressions(node: Node, scope: &str, ctx: &mut CsCtx) {
    match node.kind() {
        "invocation_expression" => {
            if let Some(target_name) = cs_callee_name(node, ctx.source) {
                ctx.pending.push(PendingRel {
                    source_key: scope.to_string(),
                    kind: RelationshipKind::Calls,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        "object_creation_expression" => {
            if let Some(target_name) = created_type_name(node, ctx.source) {
                ctx.pending.push(PendingRel {
                    source_key: scope.to_string(),
                    kind: RelationshipKind::Constructs,
                    target_name,
                    span: span_of(node),
                });
            }
        }
        "implicit_object_creation_expression" => {
            push_unresolved_edge(
                &mut ctx.out_rels,
                scope,
                RelationshipKind::Constructs,
                "",
                span_of(node),
                Some(TARGET_TYPED_NEW_REASON.to_string()),
            );
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk_expressions(child, scope, ctx);
    }
}

/// `field_declaration` has no `name` field of its own (design "C# Node-Kind
/// Mapping") — one `Field` symbol per `variable_declarator` inside its
/// `variable_declaration`, all sharing the declaration's visibility.
fn handle_field_decl(node: Node, container_key: Option<&str>, ctx: &mut CsCtx) {
    let visibility = visibility_of(node, ctx.source);
    let Some(var_decl) = find_child(node, "variable_declaration") else {
        return;
    };
    let mut cursor = var_decl.walk();
    for declarator in var_decl.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = declarator.child_by_field_name("name") else {
            continue;
        };
        let Ok(name) = name_node.utf8_text(ctx.source) else {
            continue;
        };
        let qualified = cs_qualified_name(
            ctx.relative_path,
            &ctx.namespace_stack,
            &ctx.type_stack,
            name,
        );
        let local_key = qualified.clone();
        let parent = ctx.type_stack.last().cloned();

        if let Some(container) = container_key {
            push_contains(
                &mut ctx.out_rels,
                container,
                &local_key,
                span_of(declarator),
            );
        }
        ctx.out.push(ExtractedSymbol {
            local_key: local_key.clone(),
            name: name.to_string(),
            qualified_name: qualified,
            kind: SymbolKind::Field,
            language: LanguageId::CSharp,
            span: span_of(declarator),
            parent,
            is_exported: false,
            signature_fingerprint: String::new(),
            visibility,
            is_partial: false,
            is_record: false,
            partial_ordinal: None,
        });
        // design "field_declaration ... attribute_list (attached to every
        // declarator)": the single `attribute_list` on `field_declaration`
        // (not on the declarator) applies to each declared name.
        collect_attributes(node, &local_key, ctx);
    }
}

fn push_contains(
    out: &mut Vec<ExtractedRelationship>,
    container_key: &str,
    target_key: &str,
    span: SourceSpan,
) {
    out.push(ExtractedRelationship {
        source_local_key: container_key.to_string(),
        target: EdgeTarget::Local(target_key.to_string()),
        kind: RelationshipKind::Contains,
        span,
        provenance: Provenance::Extracted,
        confidence: Confidence::Exact,
        reason: None,
    });
}

/// `using_directive` → `Imports` (design "C# Node-Kind Mapping"). Handles all
/// three forms without going through `PendingRel`/`resolve_pending` — a
/// `using` target is a namespace, never a same-file symbol, and PR5's
/// `resolve_using` looks it up by the full dotted text preserved here:
/// - `using X.Y;` → target `X.Y`, `reason: None`
/// - `using static X.Y;` → target `X.Y`, `reason: Some("static")`
/// - `using Alias = X.Y;` → target `X.Y`, `reason: Some("alias:Alias")`
/// - `global using X.Y;` → the `global` prefix is ignored (design "C#
///   Node-Kind Mapping" — ignored, not a distinct form)
fn collect_using(node: Node, ctx: &mut CsCtx) {
    let is_static = has_child_kind(node, "static");
    let alias_node = node.child_by_field_name("name");
    let mut cursor = node.walk();
    let target_node = node
        .named_children(&mut cursor)
        .find(|c| alias_node.map(|a| a.id() != c.id()).unwrap_or(true));
    let Some(target_node) = target_node else {
        return;
    };
    let Ok(target_text) = target_node.utf8_text(ctx.source) else {
        return;
    };
    let reason = match alias_node.and_then(|n| n.utf8_text(ctx.source).ok()) {
        Some(alias) => Some(format!("alias:{alias}")),
        None if is_static => Some("static".to_string()),
        None => None,
    };
    push_unresolved_edge(
        &mut ctx.out_rels,
        ctx.relative_path,
        RelationshipKind::Imports,
        target_text,
        span_of(node),
        reason,
    );
}

/// `base_list` → one entry per base (design "Architecture Decisions" — never
/// routed through `resolve_pending`; PR5's `resolve.rs` reclassifies each
/// entry to `Inherits`/`Implements` from the resolved candidate's own
/// `SymbolKind`, never guessed here by an `I`-prefix naming convention).
/// `primary_constructor_base_type` (a record's `: Base(args)` primary
/// constructor call) carries its type in a `type` field instead of being the
/// type node itself; everything else in a `base_list` names the base
/// directly.
fn collect_bases(node: Node, source_key: &str, ctx: &mut CsCtx) {
    let Some(base_list) = find_child(node, "base_list") else {
        return;
    };
    let mut cursor = base_list.walk();
    for entry in base_list.named_children(&mut cursor) {
        let type_node = match entry.kind() {
            "primary_constructor_base_type" => entry.child_by_field_name("type"),
            "argument_list" => None,
            _ => Some(entry),
        };
        let Some(type_node) = type_node else {
            continue;
        };
        let Some(name) = cs_simple_type_name(type_node, ctx.source) else {
            continue;
        };
        push_unresolved_edge(
            &mut ctx.out_rels,
            source_key,
            RelationshipKind::UsesType,
            &name,
            span_of(entry),
            Some(BASE_LIST_REASON.to_string()),
        );
    }
}

/// Each `attribute` inside every `attribute_list` child of `node` →
/// `Decorates` (design "C# Node-Kind Mapping" — never deferred through
/// `PendingRel`, target is always `Unresolved(<attribute name>)`; no
/// framework-specific handling, `[HttpGet]`/`[Obsolete]`/a custom attribute
/// all take the same path). `span` is the individual attribute's own span,
/// not the enclosing `attribute_list`'s.
fn collect_attributes(node: Node, source_key: &str, ctx: &mut CsCtx) {
    let mut cursor = node.walk();
    let attribute_lists: Vec<Node> = node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "attribute_list")
        .collect();
    for attribute_list in attribute_lists {
        let mut inner = attribute_list.walk();
        for attribute in attribute_list.named_children(&mut inner) {
            if attribute.kind() != "attribute" {
                continue;
            }
            let Some(name_node) = attribute.child_by_field_name("name") else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(ctx.source) else {
                continue;
            };
            push_unresolved_edge(
                &mut ctx.out_rels,
                source_key,
                RelationshipKind::Decorates,
                name,
                span_of(attribute),
                None,
            );
        }
    }
}

/// The callee name of an `invocation_expression` (design "C# Node-Kind
/// Mapping"): the bare identifier for `Foo()`, the accessed member for
/// `obj.Foo()`/`this.Foo()`, or a generic method's own name for `Foo<T>()`.
fn cs_callee_name(node: Node, source: &[u8]) -> Option<String> {
    let func = node.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => func.utf8_text(source).ok().map(str::to_string),
        "member_access_expression" => func
            .child_by_field_name("name")
            .and_then(|n| cs_simple_type_name(n, source)),
        "generic_name" => cs_simple_type_name(func, source),
        _ => None,
    }
}

/// The constructed type name of an `object_creation_expression` (`new Foo()`,
/// `new Ns.Foo()`, `new Foo<T>()`). `implicit_object_creation_expression`
/// (target-typed `new()`) has no `type` field and is handled separately —
/// there is no name to extract here (design "C# Node-Kind Mapping").
fn created_type_name(node: Node, source: &[u8]) -> Option<String> {
    let ty = node.child_by_field_name("type")?;
    cs_simple_type_name(ty, source)
}

/// The simple (last-segment) name of a type reference, used to match
/// same-domain project symbols by their plain `name` (design "Architecture
/// Decisions" — symbols are indexed by simple name, not a fully dotted
/// path): `identifier` → its own text; `qualified_name` → its `name` field
/// (the last segment); `generic_name` → its own identifier child, dropping
/// the type-argument list. Anything else falls back to its full source text.
fn cs_simple_type_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(source).ok().map(str::to_string),
        "qualified_name" => node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::to_string),
        "generic_name" => {
            let mut cursor = node.walk();
            let ident = node
                .named_children(&mut cursor)
                .find(|c| c.kind() == "identifier");
            ident
                .and_then(|n| n.utf8_text(source).ok())
                .map(str::to_string)
        }
        _ => node.utf8_text(source).ok().map(str::to_string),
    }
}

fn local_key_for(
    qualified_name: &str,
    partial_ordinal: Option<u32>,
    fingerprint: &str,
) -> String {
    match partial_ordinal {
        // Two partial fragments in one file share a `qualified_name`; the
        // in-memory `local_key` needs its own disambiguator too (persisted
        // identity disambiguation is `symbol_key`'s job, design "Partial
        // identity") so each fragment's own members attribute their
        // `Contains` edge to the right fragment, not either one arbitrarily.
        Some(ordinal) => format!("{qualified_name}#{ordinal}"),
        // Overloads share a `qualified_name` too; fold the signature
        // fingerprint into the local key so composition-root lookups can
        // attribute each `Contains` edge to the right member.
        None if !fingerprint.is_empty() => format!("{qualified_name}${fingerprint}"),
        None => qualified_name.to_string(),
    }
}

fn enclosing_name(ctx: &CsCtx) -> Option<String> {
    ctx.type_stack.last().cloned().or_else(|| {
        if ctx.namespace_stack.is_empty() {
            None
        } else {
            Some(ctx.namespace_stack.join("."))
        }
    })
}

/// design "qualified_name": `relative_path::Namespace.Type.member`, the same
/// two-component shape TS uses (`path::dotted-name`).
fn cs_qualified_name(relative_path: &str, ns: &[String], types: &[String], name: &str) -> String {
    let mut segs: Vec<&str> = ns.iter().map(String::as_str).collect();
    segs.extend(types.iter().map(String::as_str));
    segs.push(name);
    format!("{relative_path}::{}", segs.join("."))
}

/// Per-file, per-`qualified_name` counter so two `partial class Invoice`
/// fragments in one file get ordinals 0 and 1 (design "Partial identity").
fn next_partial_ordinal(ctx: &mut CsCtx, qualified_name: &str) -> u32 {
    let counter = ctx
        .partial_ordinals
        .entry(qualified_name.to_string())
        .or_insert(0);
    let ordinal = *counter;
    *counter += 1;
    ordinal
}

/// Scans a declaration's `modifier` children (each one word — `protected
/// internal`/`private protected` are two separate `modifier` nodes, not one
/// compound token) and maps to `Visibility`. Compound levels are checked
/// before their single components, or `protected internal` would collapse
/// to `Protected` (design "Visibility"). No modifier written → `Default`,
/// never a guessed language-implicit default.
fn visibility_of(node: Node, source: &[u8]) -> Visibility {
    let modifiers = modifier_texts(node, source);
    let has = |m: &str| modifiers.iter().any(|x| x == m);
    if has("protected") && has("internal") {
        Visibility::ProtectedInternal
    } else if has("private") && has("protected") {
        Visibility::PrivateProtected
    } else if has("public") {
        Visibility::Public
    } else if has("protected") {
        Visibility::Protected
    } else if has("internal") {
        Visibility::Internal
    } else if has("private") {
        Visibility::Private
    } else {
        Visibility::Default
    }
}

fn has_modifier(node: Node, source: &[u8], modifier: &str) -> bool {
    modifier_texts(node, source).iter().any(|m| m == modifier)
}

fn modifier_texts(node: Node, source: &[u8]) -> Vec<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|c| c.kind() == "modifier")
        .filter_map(|c| c.utf8_text(source).ok())
        .map(str::to_string)
        .collect()
}

/// `record struct` has no dedicated node kind in the pinned grammar — it's a
/// `record_declaration` with an unnamed `struct` token among its (all, not
/// just named) children, so this walks the full child list rather than
/// `find_child` (which only sees named nodes).
fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node.children(&mut cursor).any(|c| c.kind() == kind);
    found
}

/// Generic type parameters and `where` constraints, structural only (design
/// "Generic constraints": no edge of any kind is created from them). The
/// grammar exposes `type_parameters` as a named field only on
/// `interface_declaration`/`method_declaration`; `class`/`struct`/`record`
/// declarations carry the same `type_parameter_list` node as an unnamed
/// (positional) child instead, so it's read structurally rather than via
/// `fingerprint_fields`' field-name lookup. Likewise a method's return type
/// field is named `returns`, not `type`.
fn cs_fingerprint(node: Node, source: &[u8]) -> String {
    let mut parts = Vec::new();
    if let Some(type_params) = node
        .child_by_field_name("type_parameters")
        .or_else(|| find_child(node, "type_parameter_list"))
    {
        parts.push(normalize_text(type_params, source));
    }
    if let Some(params) = node
        .child_by_field_name("parameters")
        .or_else(|| find_child(node, "parameter_list"))
    {
        parts.push(normalize_text(params, source));
    }
    if let Some(ty) = node
        .child_by_field_name("type")
        .or_else(|| node.child_by_field_name("returns"))
    {
        parts.push(normalize_text(ty, source));
    }
    let mut fingerprint = parts.join("\u{1f}");

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_parameter_constraints_clause" {
            if !fingerprint.is_empty() {
                fingerprint.push('\u{1f}');
            }
            fingerprint.push_str(&normalize_text(child, source));
        }
    }
    fingerprint
}

fn normalize_text(node: Node, source: &[u8]) -> String {
    node.utf8_text(source)
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols(source: &str) -> Vec<ExtractedSymbol> {
        analyze(source, "src/test.cs").unwrap().symbols
    }

    fn find<'a>(syms: &'a [ExtractedSymbol], name: &str) -> &'a ExtractedSymbol {
        syms.iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol named {name}"))
    }

    /// Spec scenario "File-scoped namespace".
    #[test]
    fn file_scoped_namespace() {
        let syms = symbols("namespace Acme.Billing;\npublic class Invoice {}\n");
        let ns = find(&syms, "Acme.Billing");
        assert_eq!(ns.kind, SymbolKind::Namespace);
        let invoice = find(&syms, "Invoice");
        assert_eq!(invoice.qualified_name, "src/test.cs::Acme.Billing.Invoice");
    }

    /// Spec scenario "Block-scoped namespace with nested class".
    #[test]
    fn block_namespace_with_nested_class() {
        let source = "namespace Acme.Billing {\n  public class Invoice {\n    private class LineItem {}\n  }\n}\n";
        let syms = symbols(source);
        let invoice = find(&syms, "Invoice");
        let line_item = find(&syms, "LineItem");
        assert_eq!(line_item.parent.as_deref(), Some("Invoice"));
        assert_eq!(
            line_item.qualified_name,
            "src/test.cs::Acme.Billing.Invoice.LineItem"
        );
        assert_eq!(invoice.parent.as_deref(), Some("Acme.Billing"));
    }

    /// Spec scenario "Enum members index as Field".
    #[test]
    fn enum_members_index_as_field() {
        let syms = symbols("public enum Status { Draft, Sent, Paid }\n");
        let status = find(&syms, "Status");
        assert_eq!(status.kind, SymbolKind::Enum);
        for member in ["Draft", "Sent", "Paid"] {
            let sym = find(&syms, member);
            assert_eq!(sym.kind, SymbolKind::Field);
            assert_eq!(sym.parent.as_deref(), Some("Status"));
        }
    }

    /// Spec scenario "Constructor, method, property, and field are all
    /// indexed".
    #[test]
    fn constructor_method_property_field_all_indexed() {
        let source = r#"
public class Widget {
    public Widget() {}
    public void Run() {}
    public int Count { get; set; }
    private string _name;
}
"#;
        let syms = symbols(source);
        assert!(syms
            .iter()
            .any(|s| s.name == "Widget" && s.kind == SymbolKind::Constructor));
        assert_eq!(find(&syms, "Run").kind, SymbolKind::Method);
        assert_eq!(find(&syms, "Count").kind, SymbolKind::Property);
        assert_eq!(find(&syms, "_name").kind, SymbolKind::Field);

        let analysis = analyze(source, "src/test.cs").unwrap();
        let contains_count = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Contains)
            .count();
        assert_eq!(contains_count, 4);
    }

    /// Task 3.11: all six visibility levels, including both compounds.
    #[test]
    fn visibility_matrix_all_six_levels() {
        let source = r#"
public class Widget {
    public int A;
    protected int B;
    internal int C;
    private int D;
    protected internal int E;
    private protected int F;
}
"#;
        let syms = symbols(source);
        assert_eq!(find(&syms, "A").visibility, Visibility::Public);
        assert_eq!(find(&syms, "B").visibility, Visibility::Protected);
        assert_eq!(find(&syms, "C").visibility, Visibility::Internal);
        assert_eq!(find(&syms, "D").visibility, Visibility::Private);
        assert_eq!(find(&syms, "E").visibility, Visibility::ProtectedInternal);
        assert_eq!(find(&syms, "F").visibility, Visibility::PrivateProtected);
    }

    /// A member with no written modifier gets `Visibility::Default`, not a
    /// guessed C#-implicit default (design "Visibility").
    #[test]
    fn no_modifier_is_default_visibility() {
        let syms = symbols("public class Widget {\n  int _n;\n}\n");
        assert_eq!(find(&syms, "_n").visibility, Visibility::Default);
    }

    /// Task 3.13: `record`/`record class` → `Class` + `is_record`; `record
    /// struct` → `Struct` + `is_record`.
    #[test]
    fn records_map_to_class_or_struct_with_is_record() {
        let syms = symbols(
            "public record class Foo(int X);\npublic record struct Bar(int Y);\npublic record Baz(int Z);\n",
        );
        let foo = find(&syms, "Foo");
        assert_eq!(foo.kind, SymbolKind::Class);
        assert!(foo.is_record);
        let bar = find(&syms, "Bar");
        assert_eq!(bar.kind, SymbolKind::Struct);
        assert!(bar.is_record);
        let baz = find(&syms, "Baz");
        assert_eq!(baz.kind, SymbolKind::Class);
        assert!(baz.is_record);

        let widget = symbols("public class Widget {}\n");
        assert!(!find(&widget, "Widget").is_record);
    }

    /// Task 3.14: generic class with a `where` constraint — fingerprint
    /// contains the type param name and the constraint text, no edge
    /// created.
    #[test]
    fn generic_constraint_recorded_in_fingerprint_no_edge() {
        let source = "public class Repository<T> where T : IComparable {}\n";
        let analysis = analyze(source, "src/test.cs").unwrap();
        let repo = find(&analysis.symbols, "Repository");
        assert!(repo.signature_fingerprint.contains('T'));
        assert!(repo.signature_fingerprint.contains("IComparable"));
        assert!(analysis
            .relationships
            .iter()
            .all(|r| r.kind != RelationshipKind::UsesType));
    }

    /// Task 3.15: two `partial class Widget` fragments in one file get
    /// distinct ordinals and distinct local keys; each keeps its own
    /// members.
    #[test]
    fn partial_fragments_in_one_file_get_distinct_ordinals() {
        let source = "partial class Widget {\n  public void A() {}\n}\npartial class Widget {\n  public void B() {}\n}\n";
        let analysis = analyze(source, "src/test.cs").unwrap();
        let widgets: Vec<&ExtractedSymbol> = analysis
            .symbols
            .iter()
            .filter(|s| s.name == "Widget")
            .collect();
        assert_eq!(widgets.len(), 2);
        assert!(widgets.iter().all(|w| w.is_partial));
        assert_eq!(widgets[0].partial_ordinal, Some(0));
        assert_eq!(widgets[1].partial_ordinal, Some(1));
        assert_ne!(widgets[0].local_key, widgets[1].local_key);
        assert_eq!(widgets[0].qualified_name, widgets[1].qualified_name);

        // Each fragment's own member is `Contains`-linked to its own
        // fragment, not the other one.
        let a_contains = analysis
            .relationships
            .iter()
            .find(|r| r.target == EdgeTarget::Local(find(&analysis.symbols, "A").local_key.clone()))
            .unwrap();
        assert_eq!(a_contains.source_local_key, widgets[0].local_key);
        let b_contains = analysis
            .relationships
            .iter()
            .find(|r| r.target == EdgeTarget::Local(find(&analysis.symbols, "B").local_key.clone()))
            .unwrap();
        assert_eq!(b_contains.source_local_key, widgets[1].local_key);
    }

    /// Non-partial types get no ordinal at all.
    #[test]
    fn non_partial_type_has_no_ordinal() {
        let syms = symbols("public class Widget {}\n");
        assert_eq!(find(&syms, "Widget").partial_ordinal, None);
    }

    fn relationships(source: &str) -> Vec<ExtractedRelationship> {
        analyze(source, "src/test.cs").unwrap().relationships
    }

    /// Task 4.6: plain/`static`/alias `using` forms each produce an
    /// `Imports` edge with the correct `reason`.
    #[test]
    fn using_directive_forms_produce_imports_with_reason() {
        let rels = relationships(
            "using System.Collections.Generic;\nusing static System.Console;\nusing Alias = System.Collections.Generic.List<int>;\n",
        );
        let imports: Vec<&ExtractedRelationship> = rels
            .iter()
            .filter(|r| r.kind == RelationshipKind::Imports)
            .collect();
        assert_eq!(imports.len(), 3);

        let plain = imports
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("System.Collections.Generic".to_string()))
            .unwrap();
        assert_eq!(plain.reason, None);

        let static_import = imports
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("System.Console".to_string()))
            .unwrap();
        assert_eq!(static_import.reason.as_deref(), Some("static"));

        let alias_import = imports
            .iter()
            .find(|r| {
                r.target
                    == EdgeTarget::Unresolved("System.Collections.Generic.List<int>".to_string())
            })
            .unwrap();
        assert_eq!(alias_import.reason.as_deref(), Some("alias:Alias"));
    }

    /// `global using X.Y;` — the `global` prefix is ignored, still an
    /// `Imports` edge with `reason: None`.
    #[test]
    fn global_using_prefix_is_ignored() {
        let rels = relationships("global using System.Linq;\n");
        let import = rels
            .iter()
            .find(|r| r.kind == RelationshipKind::Imports)
            .unwrap();
        assert_eq!(
            import.target,
            EdgeTarget::Unresolved("System.Linq".to_string())
        );
        assert_eq!(import.reason, None);
    }

    /// Task 4.7: `public class Invoice : BillingDocument, IBillable,
    /// IAuditable` emits three independent pending base-list references,
    /// each `Unresolved` + `UsesType` + `BASE_LIST_REASON` — never routed
    /// through `resolve_pending`, never guessed as `Inherits`/`Implements`
    /// here.
    #[test]
    fn base_list_emits_one_pending_reference_per_entry() {
        let rels =
            relationships("public class Invoice : BillingDocument, IBillable, IAuditable {}\n");
        let bases: Vec<&ExtractedRelationship> = rels
            .iter()
            .filter(|r| r.kind == RelationshipKind::UsesType)
            .collect();
        assert_eq!(bases.len(), 3);
        for base in &bases {
            assert_eq!(base.confidence, Confidence::Unresolved);
            assert_eq!(base.reason.as_deref(), Some(BASE_LIST_REASON));
        }
        let targets: Vec<String> = bases
            .iter()
            .map(|r| match &r.target {
                EdgeTarget::Unresolved(name) => name.clone(),
                other => panic!("expected Unresolved target, got {other:?}"),
            })
            .collect();
        assert!(targets.contains(&"BillingDocument".to_string()));
        assert!(targets.contains(&"IBillable".to_string()));
        assert!(targets.contains(&"IAuditable".to_string()));
    }

    /// Task 4.8: direct invocation resolves to a same-file `Calls` edge;
    /// `new Foo()` resolves to a same-file `Constructs` edge; target-typed
    /// `new()` emits an unresolved `Constructs` edge with an explicit reason.
    #[test]
    fn calls_constructs_and_target_typed_new() {
        let source = r#"
public class Foo {}
public class Widget {
    public void Run() {
        Helper();
        var a = new Foo();
        Foo b = new();
    }
    private void Helper() {}
}
"#;
        let analysis = analyze(source, "src/test.cs").unwrap();

        let call = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::Calls)
            .unwrap();
        assert_eq!(
            call.target,
            EdgeTarget::Local(find(&analysis.symbols, "Helper").local_key.clone())
        );
        assert_eq!(call.confidence, Confidence::Exact);

        let constructs: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Constructs)
            .collect();
        assert_eq!(constructs.len(), 2);

        let resolved_construct = constructs
            .iter()
            .find(|r| {
                r.target == EdgeTarget::Local(find(&analysis.symbols, "Foo").local_key.clone())
            })
            .expect("new Foo() resolves to the same-file Foo class");
        assert_eq!(resolved_construct.confidence, Confidence::Exact);

        let unresolved_construct = constructs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved(String::new()))
            .expect("target-typed new() emits an unresolved Constructs edge");
        assert_eq!(
            unresolved_construct.reason.as_deref(),
            Some(TARGET_TYPED_NEW_REASON)
        );
    }

    /// Task 4.9: attributes on a declaration produce `Decorates` edges whose
    /// target text is the attribute name as written and whose span covers
    /// only the attribute, not the declaration; `[HttpGet]` is recorded
    /// literally with no framework semantics inferred (same path as
    /// `[Serializable]`/a custom attribute).
    #[test]
    fn attributes_produce_decorates_with_own_span() {
        let source = "[Serializable]\n[HttpGet]\npublic class Widget {}\n";
        let analysis = analyze(source, "src/test.cs").unwrap();
        let widget = find(&analysis.symbols, "Widget");

        let decorates: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Decorates)
            .collect();
        assert_eq!(decorates.len(), 2);

        let serializable = decorates
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("Serializable".to_string()))
            .unwrap();
        assert_eq!(serializable.source_local_key, widget.local_key);
        // The attribute's own span covers only `[Serializable]`, not the
        // whole (much larger) class declaration.
        assert!(
            serializable.span.end_byte - serializable.span.start_byte
                < widget.span.end_byte - widget.span.start_byte
        );

        let http_get = decorates
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("HttpGet".to_string()))
            .unwrap();
        assert_eq!(http_get.source_local_key, widget.local_key);
    }

    /// Attributes on a method, and on every declarator of a multi-name field
    /// declaration, still produce one `Decorates` edge per declaration/name.
    #[test]
    fn attributes_on_method_and_field_declarators() {
        let source = r#"
public class Widget {
    [Obsolete]
    public void Run() {}

    [Required]
    public int A, B;
}
"#;
        let analysis = analyze(source, "src/test.cs").unwrap();
        let run = find(&analysis.symbols, "Run");
        let a = find(&analysis.symbols, "A");
        let b = find(&analysis.symbols, "B");

        let decorates: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Decorates)
            .collect();
        assert_eq!(decorates.len(), 3);
        assert!(decorates
            .iter()
            .any(|r| r.source_local_key == run.local_key));
        assert!(decorates.iter().any(|r| r.source_local_key == a.local_key));
        assert!(decorates.iter().any(|r| r.source_local_key == b.local_key));
    }
}
