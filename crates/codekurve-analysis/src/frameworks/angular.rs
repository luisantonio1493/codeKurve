//! Angular catalogue (PR4, design "Q1 — Angular and .NET DI inference",
//! "Q5 — Array-literal shape, applied to Angular"). Consumed only by
//! `frameworks::recognize` for `LanguageId::TypeScript`/`JavaScript`, on the
//! same marker-gated re-parse `mod.rs` already produced (D1) — this module
//! never re-parses anything itself, it only walks the `Tree` it is handed.
//!
//! Every edge emitted here carries `Provenance::Heuristic` and a non-`Exact`
//! confidence (D5); `resolve.rs`'s D5 floor (PR3) is what keeps a
//! single-candidate resolution from ever being upgraded past that ceiling.

use std::collections::HashSet;

use codekurve_core::{
    Confidence, FrameworkRole, Provenance, RelationshipKind, SourceSpan, SymbolKind, Visibility,
};
use tree_sitter::{Node, Tree};

use crate::extract::span_of;
use crate::ir::{
    EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis, UnresolvedReference,
};

use super::{object_literal_entries, AttrPattern};

/// D8: Angular's closed decorator list, reused as `AttrPattern`s (design D8:
/// "Angular reuses `AttrPattern` for decorators") rather than a bespoke
/// string match — the literal-name-only matcher is exactly what this needs.
const COMPONENT: AttrPattern = AttrPattern { name: "Component" };
const DIRECTIVE: AttrPattern = AttrPattern { name: "Directive" };
const PIPE: AttrPattern = AttrPattern { name: "Pipe" };
const INJECTABLE: AttrPattern = AttrPattern { name: "Injectable" };
const NG_MODULE: AttrPattern = AttrPattern { name: "NgModule" };
const INJECT: AttrPattern = AttrPattern { name: "Inject" };

/// Entry point consumed by `frameworks::recognize` (D1/D3): walks the
/// already-parsed `tree`, role-tags decorated classes, infers `Injects` from
/// DI-host constructors/`inject()` calls (Q1), and extracts
/// providers/imports/routes array literals via the shared `object_literal_
/// entries` walker (Q5/D14).
pub(crate) fn recognize(
    tree: &Tree,
    source: &str,
    relative_path: &str,
    analysis: &mut FileAnalysis,
) {
    let bytes = source.as_bytes();
    walk(tree.root_node(), relative_path, bytes, analysis);
}

fn walk(node: Node, relative_path: &str, source: &[u8], analysis: &mut FileAnalysis) {
    match node.kind() {
        "class_declaration" | "abstract_class_declaration" => {
            process_class(node, relative_path, source, analysis);
        }
        "lexical_declaration" | "variable_declaration" => {
            process_routes_declaration(node, relative_path, source, analysis);
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, relative_path, source, analysis);
    }
}

// --- class role tagging + DI (Q1) -------------------------------------------

/// A class's own decorator list, or — when it has none — its enclosing
/// `export_statement`'s decorator list. `export class Foo {}` moves the
/// class's decorator(s) onto `export_statement`'s `decorator` field rather
/// than `class_declaration`'s own (task 2.1's grammar note, PR2), so an
/// exported `@Component`/`@Injectable` class would otherwise be silently
/// missed.
fn class_decorators(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    let own: Vec<Node> = node
        .children_by_field_name("decorator", &mut cursor)
        .collect();
    if !own.is_empty() {
        return own;
    }
    match node.parent() {
        Some(parent) if parent.kind() == "export_statement" => {
            let mut pcursor = parent.walk();
            parent
                .children_by_field_name("decorator", &mut pcursor)
                .collect()
        }
        _ => Vec::new(),
    }
}

fn process_class(node: Node, relative_path: &str, source: &[u8], analysis: &mut FileAnalysis) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Ok(class_name) = name_node.utf8_text(source) else {
        return;
    };
    let class_key = qualified_name(relative_path, class_name);
    let decorators = class_decorators(node);

    // Task 4.1: role tagging from the closed decorator list.
    let mut roles: Vec<FrameworkRole> = Vec::new();
    for decorator in &decorators {
        let Some(dname) = decorator_name(*decorator, source) else {
            continue;
        };
        if COMPONENT.matches(&dname) {
            roles.push(FrameworkRole::Component);
        } else if DIRECTIVE.matches(&dname) || PIPE.matches(&dname) {
            roles.push(FrameworkRole::Decorator);
        } else if INJECTABLE.matches(&dname) {
            roles.push(FrameworkRole::Service);
            if class_name.ends_with("Repository") || class_name.ends_with("Store") {
                roles.push(FrameworkRole::Repository);
            }
        }
    }
    if !roles.is_empty() {
        roles.sort();
        roles.dedup();
        apply_roles(analysis, class_name, &roles);
    }

    // Task 4.7: `@Component`/`@NgModule` metadata (`providers`/`imports`,
    // `HTTP_INTERCEPTORS`) — independent of the DI-host precondition below,
    // since `providers`/`imports` are registrations, not injections.
    for decorator in &decorators {
        let Some(dname) = decorator_name(*decorator, source) else {
            continue;
        };
        if COMPONENT.matches(&dname) || NG_MODULE.matches(&dname) {
            if let Some(metadata) = decorator_metadata_object(*decorator) {
                process_metadata(metadata, &class_key, source, analysis);
            }
        }
    }

    // Task 4.2: the DI-host precondition — an ordinary class constructor
    // (no matching role) emits nothing, which is the false-positive guard.
    if is_di_host(&roles) {
        process_di(node, &class_key, source, analysis);
    }
}

fn is_di_host(roles: &[FrameworkRole]) -> bool {
    roles.iter().any(|r| {
        matches!(
            r,
            FrameworkRole::Component
                | FrameworkRole::Service
                | FrameworkRole::Controller
                | FrameworkRole::Repository
                | FrameworkRole::Decorator
        )
    })
}

fn apply_roles(analysis: &mut FileAnalysis, name: &str, roles: &[FrameworkRole]) {
    if let Some(sym) = analysis
        .symbols
        .iter_mut()
        .find(|s| s.name == name && s.kind == SymbolKind::Class)
    {
        let mut merged = sym.roles.clone();
        merged.extend_from_slice(roles);
        merged.sort();
        merged.dedup();
        sym.roles = merged;
    }
}

/// Task 4.3-4.6: the DI ladder for a class already proven to be a DI host.
fn process_di(class_node: Node, class_key: &str, source: &[u8], analysis: &mut FileAnalysis) {
    let Some(body) = class_node.child_by_field_name("body") else {
        return;
    };

    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        if member.kind() == "method_definition" && is_constructor(member, source) {
            process_constructor_params(member, class_key, source, analysis);
        }
    }

    // `inject(X)` bare-identifier calls anywhere in the class body (field
    // initializer or constructor body, task 4.4) — a manual recursion
    // confined to this class body, not the shared `walk` above.
    let mut icursor = body.walk();
    for child in body.named_children(&mut icursor) {
        collect_inject_calls(child, class_key, source, analysis);
    }
}

fn is_constructor(node: Node, source: &[u8]) -> bool {
    node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source).ok())
        == Some("constructor")
}

fn process_constructor_params(
    ctor: Node,
    class_key: &str,
    source: &[u8],
    analysis: &mut FileAnalysis,
) {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for (index, param) in params.named_children(&mut cursor).enumerate() {
        if !matches!(param.kind(), "required_parameter" | "optional_parameter") {
            continue;
        }

        // Task 4.5: `@Inject(TOKEN)` overrides the declared type.
        let mut dcursor = param.walk();
        let inject_decorator = param
            .children_by_field_name("decorator", &mut dcursor)
            .find(|d| decorator_name(*d, source).is_some_and(|n| INJECT.matches(&n)));
        if let Some(decorator) = inject_decorator {
            match inject_call_token(decorator, source) {
                Some(token) => push_heuristic_edge(
                    analysis,
                    class_key,
                    RelationshipKind::Injects,
                    &token,
                    span_of(decorator),
                    Confidence::Medium,
                    Some(format!("di:token:{index}")),
                ),
                None => push_unresolved(
                    analysis,
                    class_key,
                    RelationshipKind::Injects,
                    "@Inject",
                    &format!("di:token:{index}: token is not a bare identifier"),
                ),
            }
            continue;
        }

        // Task 4.3/4.6: an explicit, non-builtin, non-generic, non-union
        // declared type; everything else is a documented no-edge case.
        match param_di_type(param, source) {
            Some(type_name) => push_heuristic_edge(
                analysis,
                class_key,
                RelationshipKind::Injects,
                &type_name,
                span_of(param),
                Confidence::High,
                Some(format!("di:ctor-param:{index}")),
            ),
            None => push_unresolved(
                analysis,
                class_key,
                RelationshipKind::Injects,
                param.utf8_text(source).unwrap_or("").trim(),
                &format!("di:ctor-param:{index}: {}", di_reject_reason(param, source)),
            ),
        }
    }
}

/// TS predefined/compound type shapes (`predefined_type`, `union_type`, ...)
/// are already rejected by `param_di_type` returning `None` for anything but
/// a bare `type_identifier`; `Date` is the one built-in that the grammar
/// still parses as a `type_identifier` (design Q1's explicit builtin list),
/// so it needs its own name check.
const BUILTIN_TYPE_NAMES: &[&str] = &["Date"];

fn param_di_type(param: Node, source: &[u8]) -> Option<String> {
    let type_ann = param.child_by_field_name("type")?;
    let ty = type_ann.named_child(0)?;
    if ty.kind() != "type_identifier" {
        return None;
    }
    let name = ty.utf8_text(source).ok()?.to_string();
    if BUILTIN_TYPE_NAMES.contains(&name.as_str()) {
        return None;
    }
    Some(name)
}

fn di_reject_reason(param: Node, source: &[u8]) -> &'static str {
    match param
        .child_by_field_name("type")
        .and_then(|t| t.named_child(0))
    {
        None => "no type annotation",
        Some(ty) => match ty.kind() {
            "predefined_type" => "primitive/builtin type",
            "union_type" | "intersection_type" => "union/intersection type",
            "literal_type" => "literal type",
            "object_type" => "anonymous type",
            "generic_type" => "generic instantiation",
            "type_identifier" if ty.utf8_text(source).ok() == Some("Date") => "builtin type name",
            _ => "unsupported type shape",
        },
    }
}

/// `@Inject(TOKEN)`'s argument, when it is a bare identifier (task 4.5/4.6).
fn inject_call_token(decorator: Node, source: &[u8]) -> Option<String> {
    let inner = decorator.named_child(0)?;
    if inner.kind() != "call_expression" {
        return None;
    }
    let args = inner.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).next()?;
    if first.kind() == "identifier" {
        first.utf8_text(source).ok().map(str::to_string)
    } else {
        None
    }
}

fn collect_inject_calls(node: Node, class_key: &str, source: &[u8], analysis: &mut FileAnalysis) {
    if node.kind() == "call_expression" {
        if let Some(func) = node.child_by_field_name("function") {
            if func.kind() == "identifier" && func.utf8_text(source) == Ok("inject") {
                if let Some(args) = node.child_by_field_name("arguments") {
                    let mut acursor = args.walk();
                    let first_arg = args.named_children(&mut acursor).next();
                    match first_arg {
                        Some(first) if first.kind() == "identifier" => {
                            if let Ok(name) = first.utf8_text(source) {
                                push_heuristic_edge(
                                    analysis,
                                    class_key,
                                    RelationshipKind::Injects,
                                    name,
                                    span_of(node),
                                    Confidence::High,
                                    Some("di:inject-fn".to_string()),
                                );
                            }
                        }
                        Some(_) => push_unresolved(
                            analysis,
                            class_key,
                            RelationshipKind::Injects,
                            "inject()",
                            "di:inject-fn: argument is not a bare identifier",
                        ),
                        None => {}
                    }
                }
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_inject_calls(child, class_key, source, analysis);
    }
}

// --- decorator metadata: providers/imports/HTTP_INTERCEPTORS (task 4.7) ----

fn decorator_metadata_object(decorator: Node) -> Option<Node> {
    let inner = decorator.named_child(0)?;
    if inner.kind() != "call_expression" {
        return None;
    }
    let args = inner.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let found = args
        .named_children(&mut cursor)
        .find(|c| c.kind() == "object");
    found
}

fn process_metadata(metadata: Node, class_key: &str, source: &[u8], analysis: &mut FileAnalysis) {
    let interceptor_spans = http_interceptor_spans(metadata, source);
    for meta in object_literal_entries(metadata, source) {
        match meta.key.as_str() {
            "providers" => {
                for entry in &meta.entries {
                    let reason = if interceptor_spans
                        .contains(&(entry.span.start_byte, entry.span.end_byte))
                    {
                        "key:providers:HTTP_INTERCEPTORS"
                    } else {
                        "key:providers"
                    };
                    push_heuristic_edge(
                        analysis,
                        class_key,
                        RelationshipKind::RegisteredAs,
                        &entry.name,
                        entry.span,
                        Confidence::High,
                        Some(reason.to_string()),
                    );
                }
            }
            "imports" => {
                for entry in &meta.entries {
                    push_heuristic_edge(
                        analysis,
                        class_key,
                        RelationshipKind::RegisteredAs,
                        &entry.name,
                        entry.span,
                        Confidence::High,
                        Some("key:imports".to_string()),
                    );
                }
            }
            _ => {}
        }
    }
}

/// Spans (by byte range) of every `providers` array element shaped
/// `{ provide: HTTP_INTERCEPTORS, ... }` — a targeted raw-node check, not a
/// second array-literal walker: `object_literal_entries` already resolved
/// the entry's *name* (the `useClass` value); this only tags *which*
/// resolved entries came from that one specific token so their `reason` can
/// say so (design Q5 table).
fn http_interceptor_spans(metadata: Node, source: &[u8]) -> HashSet<(usize, usize)> {
    let mut spans = HashSet::new();
    let mut cursor = metadata.walk();
    for pair in metadata
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "pair")
    {
        let Some(key_node) = pair.child_by_field_name("key") else {
            continue;
        };
        if key_node.utf8_text(source) != Ok("providers") {
            continue;
        }
        let Some(value) = pair.child_by_field_name("value") else {
            continue;
        };
        if value.kind() != "array" {
            continue;
        }
        let mut vcursor = value.walk();
        for element in value.named_children(&mut vcursor) {
            if element.kind() != "object" {
                continue;
            }
            let mut ecursor = element.walk();
            let is_interceptor = element
                .named_children(&mut ecursor)
                .filter(|c| c.kind() == "pair")
                .any(|p| {
                    let (Some(k), Some(v)) =
                        (p.child_by_field_name("key"), p.child_by_field_name("value"))
                    else {
                        return false;
                    };
                    k.utf8_text(source) == Ok("provide")
                        && v.utf8_text(source)
                            .map(|t| t.trim() == "HTTP_INTERCEPTORS")
                            .unwrap_or(false)
                });
            if is_interceptor {
                spans.insert((element.start_byte(), element.end_byte()));
            }
        }
    }
    spans
}

// --- routes array (task 4.8/4.9) --------------------------------------------

/// A `const`/`let`/`var` declarator assigned an array literal, whose name or
/// type annotation says "Routes" (design Q5 table). No `ExtractedSymbol` for
/// a top-level variable exists yet (the TS analyzer extracts none per its
/// current scope) — this is the one place `frameworks::recognize` appends a
/// new symbol rather than only tagging an existing one, because the source
/// of a route's `HandlesRoute` edge must be a real stored symbol.
fn process_routes_declaration(
    node: Node,
    relative_path: &str,
    source: &[u8],
    analysis: &mut FileAnalysis,
) {
    let mut cursor = node.walk();
    for declarator in node.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let Some(name_node) = declarator.child_by_field_name("name") else {
            continue;
        };
        let Ok(name) = name_node.utf8_text(source) else {
            continue;
        };

        let is_routes_typed = declarator
            .child_by_field_name("type")
            .and_then(|t| t.named_child(0))
            .and_then(|t| t.utf8_text(source).ok())
            == Some("Routes");
        let is_routes_named = name.to_lowercase().contains("routes");
        if !is_routes_typed && !is_routes_named {
            continue;
        }

        let Some(value) = declarator.child_by_field_name("value") else {
            continue;
        };
        if value.kind() != "array" {
            continue;
        }

        let var_key = ensure_variable_symbol(analysis, relative_path, name, span_of(declarator));
        let mut vcursor = value.walk();
        for element in value.named_children(&mut vcursor) {
            if element.kind() == "object" {
                process_route_object(element, &var_key, "", source, analysis);
            }
        }
    }
}

fn ensure_variable_symbol(
    analysis: &mut FileAnalysis,
    relative_path: &str,
    name: &str,
    span: SourceSpan,
) -> String {
    let key = qualified_name(relative_path, name);
    if !analysis.symbols.iter().any(|s| s.local_key == key) {
        analysis.symbols.push(ExtractedSymbol {
            local_key: key.clone(),
            name: name.to_string(),
            qualified_name: key.clone(),
            kind: SymbolKind::Variable,
            language: analysis.language,
            span,
            parent: None,
            is_exported: false,
            signature_fingerprint: String::new(),
            visibility: Visibility::Default,
            is_partial: false,
            is_record: false,
            partial_ordinal: None,
            roles: vec![],
        });
    }
    key
}

fn process_route_object(
    node: Node,
    source_key: &str,
    prefix: &str,
    source: &[u8],
    analysis: &mut FileAnalysis,
) {
    let keys = object_literal_entries(node, source);
    let path_segment = keys
        .iter()
        .find(|k| k.key == "path")
        .and_then(|k| k.entries.first())
        .map(|e| e.name.as_str())
        .unwrap_or("");
    let full_path = join_route_path(prefix, path_segment);

    if let Some(component) = keys
        .iter()
        .find(|k| k.key == "component")
        .and_then(|k| k.entries.first())
    {
        push_heuristic_edge(
            analysis,
            source_key,
            RelationshipKind::HandlesRoute,
            &component.name,
            component.span,
            Confidence::High,
            Some(format!("route:{full_path}")),
        );
    }

    if let Some(guards) = keys.iter().find(|k| k.key == "canActivate") {
        for guard in &guards.entries {
            push_heuristic_edge(
                analysis,
                source_key,
                RelationshipKind::RegisteredAs,
                &guard.name,
                guard.span,
                Confidence::High,
                Some("key:canActivate".to_string()),
            );
        }
    }

    // `loadComponent`/`loadChildren`/`children` all need the raw pair value
    // node (an arrow function, or a nested array of route objects) that
    // `object_literal_entries`'s generic name resolution deliberately
    // doesn't handle (D14's own doc comment).
    let mut cursor = node.walk();
    for pair in node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "pair")
    {
        let (Some(key_node), Some(value)) = (
            pair.child_by_field_name("key"),
            pair.child_by_field_name("value"),
        ) else {
            continue;
        };
        let Ok(key_text) = key_node.utf8_text(source) else {
            continue;
        };
        match key_text {
            "loadComponent" => match last_member_property(value, source) {
                Some(name) => push_heuristic_edge(
                    analysis,
                    source_key,
                    RelationshipKind::HandlesRoute,
                    &name,
                    span_of(value),
                    Confidence::Medium,
                    Some(format!("route:{full_path}")),
                ),
                None => push_unresolved(
                    analysis,
                    source_key,
                    RelationshipKind::HandlesRoute,
                    "loadComponent",
                    "loadComponent: no extractable member name",
                ),
            },
            // ponytail: `loadChildren` only ever emits the documented
            // no-edge case (design.md: "no extractable member name ->
            // UnresolvedReference") — child routes aren't a single
            // component, so a successful extraction has no edge shape the
            // design specifies; upgrade if a real chain needs it.
            "loadChildren" => {
                if last_member_property(value, source).is_none() {
                    push_unresolved(
                        analysis,
                        source_key,
                        RelationshipKind::HandlesRoute,
                        "loadChildren",
                        "loadChildren: no extractable member name",
                    );
                }
            }
            "children" if value.kind() == "array" => {
                let mut ccursor = value.walk();
                for child_route in value
                    .named_children(&mut ccursor)
                    .filter(|c| c.kind() == "object")
                {
                    process_route_object(child_route, source_key, &full_path, source, analysis);
                }
            }
            _ => {}
        }
    }
}

fn join_route_path(prefix: &str, segment: &str) -> String {
    let trimmed = segment.trim_matches('/');
    if trimmed.is_empty() {
        format!("{prefix}/")
    } else {
        format!("{prefix}/{trimmed}")
    }
}

/// The last dotted segment of the first `member_expression` found in
/// `node`'s subtree — e.g. `() => import('./x').then(m => m.XComponent)`'s
/// `m.XComponent` -> `"XComponent"` (design Q5's `loadComponent` rule).
fn last_member_property(node: Node, source: &[u8]) -> Option<String> {
    // The rightmost `member_expression` by source position — e.g. in
    // `import('./x').then(m => m.XComponent)` two exist (`import(...).then`
    // and `m.XComponent`); the *last* one written is the evidence (design
    // Q5's `loadComponent` rule), not the outermost/first one a preorder
    // walk would reach.
    let last = all_member_expressions(node)
        .into_iter()
        .max_by_key(|m| m.start_byte())?;
    last.child_by_field_name("property")
        .and_then(|p| p.utf8_text(source).ok())
        .map(str::to_string)
}

fn all_member_expressions(node: Node) -> Vec<Node> {
    let mut found = Vec::new();
    if node.kind() == "member_expression" {
        found.push(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        found.extend(all_member_expressions(child));
    }
    found
}

// --- shared helpers ----------------------------------------------------------

fn qualified_name(relative_path: &str, name: &str) -> String {
    format!("{relative_path}::{name}")
}

/// The literal decorator name text — mirrors `languages/typescript.rs`'s
/// private `decorator_name`/`decorator_expr_name` (task 2.2's grammar
/// shapes), duplicated here rather than shared because that function is
/// framework-blind on purpose (D1: no framework name may live under
/// `languages/`) and this one only ever asks about the closed Angular list.
fn decorator_name(node: Node, source: &[u8]) -> Option<String> {
    let inner = node.named_child(0)?;
    decorator_expr_name(inner, source)
}

fn decorator_expr_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(source).ok().map(str::to_string),
        "call_expression" => decorator_expr_name(node.child_by_field_name("function")?, source),
        "member_expression" => node
            .child_by_field_name("property")
            .and_then(|p| p.utf8_text(source).ok())
            .map(str::to_string),
        "parenthesized_expression" => decorator_expr_name(node.named_child(0)?, source),
        _ => node.utf8_text(source).ok().map(str::to_string),
    }
}

fn push_heuristic_edge(
    analysis: &mut FileAnalysis,
    source_key: &str,
    kind: RelationshipKind,
    target_name: &str,
    span: SourceSpan,
    confidence: Confidence,
    reason: Option<String>,
) {
    analysis.relationships.push(ExtractedRelationship {
        source_local_key: source_key.to_string(),
        target: EdgeTarget::Unresolved(target_name.to_string()),
        kind,
        span,
        provenance: Provenance::Heuristic,
        confidence,
        reason,
    });
}

fn push_unresolved(
    analysis: &mut FileAnalysis,
    source_key: &str,
    kind: RelationshipKind,
    target_text: &str,
    reason: &str,
) {
    analysis.unresolved.push(UnresolvedReference {
        source_local_key: source_key.to_string(),
        relationship_kind: kind,
        target_text: target_text.to_string(),
        context: None,
        candidate_count: 0,
        reason: reason.to_string(),
        confidence: Confidence::Unresolved,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;
    use crate::resolve;
    use codekurve_core::LanguageId;

    fn rel<'a>(
        analysis: &'a FileAnalysis,
        kind: RelationshipKind,
    ) -> Vec<&'a ExtractedRelationship> {
        analysis
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .collect()
    }

    // --- role tagging (task 4.12) -------------------------------------------

    #[test]
    fn component_decorator_tags_component_role() {
        let analysis = extract::analyze(
            "@Component({ selector: 'app-x' })\nexport class Widget {}",
            LanguageId::TypeScript,
            "src/widget.ts",
        )
        .unwrap();
        let sym = analysis
            .symbols
            .iter()
            .find(|s| s.name == "Widget")
            .unwrap();
        assert_eq!(sym.roles, vec![FrameworkRole::Component]);
    }

    #[test]
    fn directive_and_pipe_tag_decorator_role() {
        for (decorator, name) in [
            ("@Directive({})", "HighlightDirective"),
            ("@Pipe({})", "TruncatePipe"),
        ] {
            let source = format!("{decorator}\nexport class {name} {{}}");
            let analysis = extract::analyze(&source, LanguageId::TypeScript, "src/d.ts").unwrap();
            let sym = analysis.symbols.iter().find(|s| s.name == name).unwrap();
            assert_eq!(sym.roles, vec![FrameworkRole::Decorator]);
        }
    }

    #[test]
    fn injectable_tags_service_role() {
        let analysis = extract::analyze(
            "@Injectable()\nexport class InvoiceApi {}",
            LanguageId::TypeScript,
            "src/api.ts",
        )
        .unwrap();
        let sym = analysis
            .symbols
            .iter()
            .find(|s| s.name == "InvoiceApi")
            .unwrap();
        assert_eq!(sym.roles, vec![FrameworkRole::Service]);
    }

    #[test]
    fn injectable_with_repository_or_store_suffix_tags_both_roles() {
        for name in ["InvoiceRepository", "InvoiceStore"] {
            let source = format!("@Injectable()\nexport class {name} {{}}");
            let analysis =
                extract::analyze(&source, LanguageId::TypeScript, "src/repo.ts").unwrap();
            let sym = analysis.symbols.iter().find(|s| s.name == name).unwrap();
            // `FrameworkRole`'s declaration order (Service < Repository) is
            // what `roles.sort()` orders by (task 1.2's derived `Ord`).
            assert_eq!(
                sym.roles,
                vec![FrameworkRole::Service, FrameworkRole::Repository]
            );
        }
    }

    #[test]
    fn combined_component_and_injectable_yields_both_roles() {
        let analysis = extract::analyze(
            "@Component({})\n@Injectable()\nexport class Weird {}",
            LanguageId::TypeScript,
            "src/weird.ts",
        )
        .unwrap();
        let sym = analysis.symbols.iter().find(|s| s.name == "Weird").unwrap();
        assert_eq!(
            sym.roles,
            vec![FrameworkRole::Service, FrameworkRole::Component]
        );
    }

    // --- DI ladder (task 4.10) ----------------------------------------------

    #[test]
    fn di_ctor_param_with_explicit_type_on_di_host_produces_high_injects() {
        let source = r#"
@Injectable()
export class Logger {}

@Component({})
export class Widget {
  constructor(private logger: Logger) {}
}
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/widget.ts").unwrap();
        let edges = rel(&analysis, RelationshipKind::Injects);
        assert_eq!(edges.len(), 1);
        let edge = edges[0];
        assert_eq!(edge.source_local_key, "src/widget.ts::Widget");
        assert_eq!(edge.target, EdgeTarget::Unresolved("Logger".to_string()));
        assert_eq!(edge.provenance, Provenance::Heuristic);
        assert_eq!(edge.confidence, Confidence::High);
        assert_eq!(edge.reason.as_deref(), Some("di:ctor-param:0"));
    }

    #[test]
    fn inject_fn_bare_identifier_in_di_host_produces_high_injects() {
        let source = r#"
@Injectable()
export class Logger {}

@Injectable()
export class Widget {
  private logger = inject(Logger);
}
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/w.ts").unwrap();
        let edges = rel(&analysis, RelationshipKind::Injects);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].reason.as_deref(), Some("di:inject-fn"));
        assert_eq!(edges[0].confidence, Confidence::High);
        assert_eq!(edges[0].provenance, Provenance::Heuristic);
    }

    #[test]
    fn inject_token_decorator_overrides_type_at_medium_confidence() {
        let source = r#"
@Component({})
export class Widget {
  constructor(@Inject(APP_CONFIG) private config: Config) {}
}
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/w2.ts").unwrap();
        let edges = rel(&analysis, RelationshipKind::Injects);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].target,
            EdgeTarget::Unresolved("APP_CONFIG".to_string())
        );
        assert_eq!(edges[0].confidence, Confidence::Medium);
        assert_eq!(edges[0].reason.as_deref(), Some("di:token:0"));
    }

    #[test]
    fn non_di_host_class_constructor_emits_nothing() {
        let source = r#"
@Injectable()
export class Logger {}

export class Plain {
  constructor(private logger: Logger) {}
}
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/plain.ts").unwrap();
        assert!(rel(&analysis, RelationshipKind::Injects).is_empty());
        assert!(analysis
            .unresolved
            .iter()
            .all(|u| u.source_local_key != "src/plain.ts::Plain"));
    }

    #[test]
    fn every_di_no_edge_shape_emits_an_unresolved_reference_not_an_edge() {
        let source = r#"
@Injectable()
export class Widget {
  constructor(
    a,
    b: string,
    c: Foo | Bar,
    d: Array<Foo>,
    e: Date,
  ) {}
}
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/nodi.ts").unwrap();
        assert!(rel(&analysis, RelationshipKind::Injects).is_empty());
        let unresolved: Vec<_> = analysis
            .unresolved
            .iter()
            .filter(|u| u.relationship_kind == RelationshipKind::Injects)
            .collect();
        assert_eq!(unresolved.len(), 5);
    }

    #[test]
    fn inject_with_non_identifier_argument_is_unresolved_not_an_edge() {
        let source = r#"
@Injectable()
export class Widget {
  private cfg = inject('token');
}
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/injnonid.ts").unwrap();
        assert!(rel(&analysis, RelationshipKind::Injects).is_empty());
        assert!(analysis
            .unresolved
            .iter()
            .any(|u| u.relationship_kind == RelationshipKind::Injects
                && u.reason.contains("di:inject-fn")));
    }

    #[test]
    fn inject_token_decorator_with_non_identifier_token_is_unresolved() {
        let source = r#"
@Component({})
export class Widget {
  constructor(@Inject('literal-token') private config: Config) {}
}
"#;
        let analysis =
            extract::analyze(source, LanguageId::TypeScript, "src/injliteral.ts").unwrap();
        assert!(rel(&analysis, RelationshipKind::Injects).is_empty());
        assert!(analysis
            .unresolved
            .iter()
            .any(|u| u.relationship_kind == RelationshipKind::Injects
                && u.reason.contains("di:token:0")));
    }

    // --- negative fixture (task 4.11) ---------------------------------------

    #[test]
    fn negative_fixture_same_named_non_injectable_class_produces_two_low_edges_not_one_high() {
        let injectable_source = "@Injectable()\nexport class Foo {}";
        let plain_source = "export class Foo {}";
        let host_source = r#"
@Component({})
export class Widget {
  constructor(private foo: Foo) {}
}
"#;
        let mut files = vec![
            extract::analyze(
                injectable_source,
                LanguageId::TypeScript,
                "src/injectable-foo.ts",
            )
            .unwrap(),
            extract::analyze(plain_source, LanguageId::TypeScript, "src/plain-foo.ts").unwrap(),
            extract::analyze(host_source, LanguageId::TypeScript, "src/widget2.ts").unwrap(),
        ];
        resolve::resolve(&mut files, &Default::default());

        let widget = files.iter().find(|f| f.file == "src/widget2.ts").unwrap();
        let injects: Vec<_> = widget
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Injects)
            .collect();
        assert_eq!(
            injects.len(),
            2,
            "two same-named candidates must never collapse to one"
        );
        for edge in &injects {
            assert_eq!(edge.confidence, Confidence::Low);
            assert_eq!(edge.provenance, Provenance::Heuristic);
        }
    }

    // --- providers/imports/HTTP_INTERCEPTORS (task 4.7/4.14) ----------------

    #[test]
    fn ng_module_providers_and_imports_produce_registered_as_edges() {
        let source = r#"
@NgModule({
  providers: [FooService, { provide: TOKEN, useClass: BarService }],
  imports: [SharedModule],
})
export class AppModule {}
"#;
        let analysis =
            extract::analyze(source, LanguageId::TypeScript, "src/app.module.ts").unwrap();
        let regs = rel(&analysis, RelationshipKind::RegisteredAs);
        let providers: Vec<_> = regs
            .iter()
            .filter(|r| r.reason.as_deref() == Some("key:providers"))
            .collect();
        assert_eq!(providers.len(), 2);
        let imports: Vec<_> = regs
            .iter()
            .filter(|r| r.reason.as_deref() == Some("key:imports"))
            .collect();
        assert_eq!(imports.len(), 1);
        assert_eq!(
            imports[0].target,
            EdgeTarget::Unresolved("SharedModule".to_string())
        );
    }

    #[test]
    fn http_interceptors_provider_resolves_to_use_class_with_dedicated_reason() {
        let source = r#"
@NgModule({
  providers: [
    { provide: HTTP_INTERCEPTORS, useClass: AuthInterceptor, multi: true },
  ],
})
export class AppModule {}
"#;
        let analysis =
            extract::analyze(source, LanguageId::TypeScript, "src/app2.module.ts").unwrap();
        let regs = rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 1);
        assert_eq!(
            regs[0].target,
            EdgeTarget::Unresolved("AuthInterceptor".to_string())
        );
        assert_eq!(
            regs[0].reason.as_deref(),
            Some("key:providers:HTTP_INTERCEPTORS")
        );
    }

    // --- routes array (task 4.13/4.9) ---------------------------------------

    #[test]
    fn routes_array_with_guard_and_nested_children_produces_prefixed_edges() {
        let source = r#"
const routes: Routes = [
  {
    path: 'admin',
    canActivate: [AuthGuard],
    children: [
      { path: 'users', component: UsersComponent },
    ],
  },
];
"#;
        let analysis =
            extract::analyze(source, LanguageId::TypeScript, "src/app.routes.ts").unwrap();

        let handles = rel(&analysis, RelationshipKind::HandlesRoute);
        assert_eq!(handles.len(), 1);
        assert_eq!(
            handles[0].target,
            EdgeTarget::Unresolved("UsersComponent".to_string())
        );
        assert_eq!(handles[0].reason.as_deref(), Some("route:/admin/users"));
        assert_eq!(handles[0].source_local_key, "src/app.routes.ts::routes");

        let guards = rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(guards.len(), 1);
        assert_eq!(
            guards[0].target,
            EdgeTarget::Unresolved("AuthGuard".to_string())
        );
        assert_eq!(guards[0].reason.as_deref(), Some("key:canActivate"));

        let sym = analysis
            .symbols
            .iter()
            .find(|s| s.local_key == "src/app.routes.ts::routes")
            .unwrap();
        assert_eq!(sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn load_component_arrow_extracts_last_member_segment_at_medium_confidence() {
        let source = r#"
const routes: Routes = [
  { path: 'lazy', loadComponent: () => import('./lazy.component').then(m => m.LazyComponent) },
];
"#;
        let analysis =
            extract::analyze(source, LanguageId::TypeScript, "src/lazy.routes.ts").unwrap();
        let handles = rel(&analysis, RelationshipKind::HandlesRoute);
        assert_eq!(handles.len(), 1);
        assert_eq!(
            handles[0].target,
            EdgeTarget::Unresolved("LazyComponent".to_string())
        );
        assert_eq!(handles[0].confidence, Confidence::Medium);
        assert_eq!(handles[0].reason.as_deref(), Some("route:/lazy"));
    }

    #[test]
    fn load_children_with_no_extractable_member_name_is_unresolved() {
        let source = r#"
const routes: Routes = [
  { path: 'admin', loadChildren: () => someDynamicThing() },
];
"#;
        let analysis =
            extract::analyze(source, LanguageId::TypeScript, "src/lc.routes.ts").unwrap();
        assert!(rel(&analysis, RelationshipKind::HandlesRoute).is_empty());
        assert!(analysis
            .unresolved
            .iter()
            .any(|u| u.target_text == "loadChildren"));
    }

    // --- provenance (task 4.15) ----------------------------------------------

    #[test]
    fn every_angular_edge_is_heuristic_never_extracted_or_resolved() {
        let source = r#"
@Injectable()
export class Logger {}

@Component({ providers: [Logger], imports: [SharedModule] })
export class Widget {
  constructor(private logger: Logger) {}
}

const routes: Routes = [{ path: 'x', component: Widget, canActivate: [AuthGuard] }];
"#;
        let analysis = extract::analyze(source, LanguageId::TypeScript, "src/all.ts").unwrap();
        let framework_kinds = [
            RelationshipKind::Injects,
            RelationshipKind::RegisteredAs,
            RelationshipKind::HandlesRoute,
        ];
        let framework_edges: Vec<_> = analysis
            .relationships
            .iter()
            .filter(|r| framework_kinds.contains(&r.kind))
            .collect();
        assert!(!framework_edges.is_empty());
        for edge in framework_edges {
            assert_eq!(edge.provenance, Provenance::Heuristic);
            assert_ne!(edge.confidence, Confidence::Exact);
        }
    }

    // --- runtime harness (task 4.16) ----------------------------------------

    #[test]
    fn runtime_harness_component_service_route_trio() {
        let component = "@Component({ selector: 'app-invoice', providers: [] })\nexport class InvoiceComponent {\n  constructor(private api: InvoiceApiService) {}\n}\n";
        let service = "@Injectable()\nexport class InvoiceApiService {}\n";
        let route_config =
            "const routes: Routes = [{ path: 'invoices', component: InvoiceComponent }];\n";

        let c = extract::analyze(
            component,
            LanguageId::TypeScript,
            "src/invoice.component.ts",
        )
        .unwrap();
        let s =
            extract::analyze(service, LanguageId::TypeScript, "src/invoice.service.ts").unwrap();
        let r =
            extract::analyze(route_config, LanguageId::TypeScript, "src/app.routes.ts").unwrap();

        assert!(c
            .symbols
            .iter()
            .any(|sym| sym.name == "InvoiceComponent"
                && sym.roles.contains(&FrameworkRole::Component)));
        assert!(s
            .symbols
            .iter()
            .any(|sym| sym.name == "InvoiceApiService"
                && sym.roles.contains(&FrameworkRole::Service)));
        assert!(c
            .relationships
            .iter()
            .any(|rel| rel.kind == RelationshipKind::Injects));
        assert!(r
            .relationships
            .iter()
            .any(|rel| rel.kind == RelationshipKind::HandlesRoute));
    }
}
