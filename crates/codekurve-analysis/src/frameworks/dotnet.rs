//! .NET catalogue (design "Module Layout"). PR5 covers the attribute-driven
//! slice — MVC controllers and Azure Functions (design "Q2 — .NET split", D7
//! slice A). PR6 (this revision) extends the same file with the call-driven
//! slice — minimal APIs, DI registrations, middleware, EF Core (design "Q2"
//! slice B, D9, D11) — the module is one closed catalogue, split by evidence
//! shape, not by file.
//!
//! Walks its own re-parsed tree (`frameworks::recognize`'s `parse`), never
//! `languages/csharp.rs`'s output (D1) — `class_declaration`/
//! `method_declaration` nodes are matched back to `analysis.symbols` by span:
//! `span_of` on the same source bytes with the same `tree-sitter-c-sharp`
//! grammar version yields identical byte offsets to what `csharp.rs` already
//! recorded, so a span match is an exact identity join, not a name guess.
//! PR6's own evidence (`AddScoped<A, B>()`'s type-argument list, `DbSet<T>`'s
//! type argument) is exactly the syntax `cs_simple_type_name`/`cs_callee_name`
//! (`languages/csharp.rs`) deliberately drop (design's corrected assumption
//! #1) — this file's own re-parse is what recovers it.

use codekurve_core::{
    Confidence, FrameworkRole, LanguageId, Provenance, RelationshipKind, SourceSpan, SymbolKind,
    Visibility,
};
use tree_sitter::Node;

use crate::extract::{find_child, span_of};
use crate::frameworks::AttrPattern;
use crate::ir::{EdgeTarget, ExtractedRelationship, ExtractedSymbol, FileAnalysis};

/// design "Q2 — .NET split" PR5 table: attribute name -> HTTP verb text.
const HTTP_VERB_ATTRS: &[(&str, &str)] = &[
    ("HttpGet", "GET"),
    ("HttpPost", "POST"),
    ("HttpPut", "PUT"),
    ("HttpDelete", "DELETE"),
    ("HttpPatch", "PATCH"),
];

/// One `attribute` node's resolved name plus its string-literal arguments
/// (in source order) — the only argument shape this PR's evidence needs
/// (`[Route("tpl")]`, `[HttpGet("tpl")]`, `[Function("Name")]`, a trigger
/// attribute's first literal argument).
#[derive(Debug, Clone)]
struct Attr {
    name: String,
    args: Vec<String>,
}

/// design "Q2 — .NET split" PR6 table: `Map*` verb -> HTTP verb text.
const MAP_VERBS: &[(&str, &str)] = &[
    ("MapGet", "GET"),
    ("MapPost", "POST"),
    ("MapPut", "PUT"),
    ("MapDelete", "DELETE"),
    ("MapPatch", "PATCH"),
];

/// design "Q2" PR6 table: `Add*` lifetime-method name -> the lowercase word
/// that rides in `reason` (`"lifetime:scoped;..."`).
const ADD_LIFETIMES: &[(&str, &str)] = &[
    ("AddScoped", "scoped"),
    ("AddTransient", "transient"),
    ("AddSingleton", "singleton"),
];

/// Closed exact-name list of ASP.NET Core / EF Core registrations whose
/// **type argument** names the registered type — the same evidence shape
/// `UseMiddleware<T>()` already uses (the type argument is the target, the
/// method name is only the discriminator). Matched by exact name, never by
/// an `Add` prefix (design "Q2" D9); a call from this list with **no** type
/// argument emits nothing, because its bare overload registers a framework
/// service rather than a project type and inventing one would be a guess.
// ponytail: a representative closed list of the standard typed registration
// APIs, not an exhaustive one — same published bound as
// `MIDDLEWARE_USE_NAMES`; extend it if a real fixture needs one more entry.
//
// Not `*_ADD_NAMES`: `UseStartup<T>` is the same *shape* (the type argument
// names a project class) despite the `Use` prefix, so the list is keyed on
// shape, not on the verb. It stays separate from `UseMiddleware`, which is
// also typed but is semantically middleware and carries the middleware
// `reason` — folding the two would relabel existing edges.
const TYPED_FEATURE_NAMES: &[&str] = &[
    "AddDbContext",
    "AddDbContextFactory",
    "AddDbContextPool",
    "AddPooledDbContextFactory",
    "AddDbContextCheck",
    "AddHostedService",
    "AddHttpClient",
    "AddDocumentTransformer",
    "AddOperationTransformer",
    "AddSchemaTransformer",
    // Classic (pre-minimal-API) host builder: `.UseStartup<Startup>()` names
    // the project's own startup class. Found on a production ASP.NET Core
    // app that predates the minimal-API style the rest of this list was
    // surveyed from.
    "UseStartup",
];

/// Closed exact-name list of ASP.NET Core feature registrations that take no
/// project type at all (`builder.Services.AddOpenApi()`) — the feature itself
/// is the target, exactly as `MIDDLEWARE_USE_NAMES` treats a `Use<Name>()`
/// call. Matched by exact name, never by an `Add` prefix (D9), which is what
/// keeps unrelated `.Add*(` calls (`AddColumn`, `AddForeignKey`, `AddTicks`,
/// `AddPolicy`, `AddAnnotation`) out of the graph.
// ponytail: a representative closed list of the standard ASP.NET Core
// service-registration call names, not an exhaustive one — same published
// bound as `MIDDLEWARE_USE_NAMES`; extend it if a real fixture needs one
// more entry.
const BARE_FEATURE_ADD_NAMES: &[&str] = &[
    "AddAntiforgery",
    "AddApiVersioning",
    "AddAuthentication",
    "AddAuthorization",
    "AddControllers",
    "AddControllersWithViews",
    "AddCors",
    "AddDatabaseDeveloperPageExceptionFilter",
    "AddDistributedMemoryCache",
    "AddEndpointsApiExplorer",
    "AddHealthChecks",
    "AddHttpContextAccessor",
    "AddMemoryCache",
    "AddMvc",
    "AddMvcCore",
    "AddOpenApi",
    "AddOutputCache",
    "AddProblemDetails",
    "AddRateLimiter",
    "AddRazorPages",
    "AddResponseCaching",
    "AddResponseCompression",
    "AddRouting",
    "AddSession",
    "AddSignalR",
    "AddSwaggerGen",
];

/// Task 6.5's closed exact-name list of ASP.NET Core middleware `Use*` calls
/// — matched by exact name, never by a `Use` prefix (design "Q2": "`Add*`
/// and `Use*` are matched by exact name from the closed list, never by
/// prefix"). `UseMiddleware<T>()` is handled separately (its evidence is the
/// type argument, not this name list).
// ponytail: a representative closed list of the standard ASP.NET Core
// middleware call names, not an exhaustive one — `AddSomethingCustom`-shaped
// misses are the published limitation task 6.11 tests for; extend this list
// if a real fixture needs one more entry.
const MIDDLEWARE_USE_NAMES: &[&str] = &[
    "UseAuthentication",
    "UseAuthorization",
    "UseCors",
    "UseHttpsRedirection",
    "UseRouting",
    "UseEndpoints",
    "UseStaticFiles",
    "UseSwagger",
    "UseSwaggerUI",
    "UseExceptionHandler",
    "UseDeveloperExceptionPage",
    "UseSession",
    "UseResponseCompression",
    "UseResponseCaching",
    "UseOutputCache",
    "UseRateLimiter",
    "UseWebSockets",
    "UseAntiforgery",
    "UseHsts",
    // Classic (pre-minimal-API) pipeline, still in production use — both
    // observed on a real ASP.NET Core app whose style predates the rest of
    // this list.
    "UseMvc",
    "UseIdentityServer",
];

/// One resolved `invocation_expression`'s callee name, its type-argument
/// list (empty when the call isn't generic), and — for a `receiver.Name(...)`
/// call — the receiver expression node itself (task 6.2's `MapGroup` chain
/// detection needs it; D9 still never matches a call by the receiver's own
/// name/type).
struct CallInfo<'a> {
    name: String,
    type_args: Vec<String>,
    receiver: Option<Node<'a>>,
}

/// Entry point, called from `frameworks::recognize` once the marker
/// prefilter and this file's own parse have already run.
pub(crate) fn recognize(root: Node, source: &[u8], analysis: &mut FileAnalysis) {
    let mut top_level_key: Option<String> = None;
    walk(root, source, analysis, None, &mut top_level_key);
}

/// `current_method` is the local key of the nearest enclosing
/// `method_declaration`/`constructor_declaration` symbol, threaded down
/// through recursion; `None` means the call site is a top-level statement
/// (`global_statement` — modern `Program.cs`'s implicit entry point, which
/// `languages/csharp.rs` extracts no symbol for at all), in which case
/// `top_level_key` lazily synthesizes one shared `Program.Main` `Method`
/// symbol per file (task 6.1's "usually `Main`/`Program`").
fn walk(
    node: Node,
    source: &[u8],
    analysis: &mut FileAnalysis,
    current_method: Option<&str>,
    top_level_key: &mut Option<String>,
) {
    let mut next_method: Option<String> = None;

    match node.kind() {
        "class_declaration" => {
            recognize_class(node, source, analysis);
            recognize_dbcontext(node, source, analysis);
        }
        "method_declaration" | "constructor_declaration" => {
            let kind = if node.kind() == "constructor_declaration" {
                SymbolKind::Constructor
            } else {
                SymbolKind::Method
            };
            next_method = find_local_key(analysis, span_of(node), kind);
        }
        "invocation_expression" => {
            let source_key = match current_method {
                Some(key) => key.to_string(),
                None => ensure_program_main_symbol(analysis, top_level_key, span_of(node)),
            };
            recognize_call(node, source, analysis, &source_key);
        }
        _ => {}
    }

    let next_method = next_method.as_deref().or(current_method);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, source, analysis, next_method, top_level_key);
    }
}

/// Task 6.1's synthetic source symbol for top-level `Program.cs` statements
/// — created once per file, on first use, and reused for every subsequent
/// top-level call-driven edge. Not something `languages/csharp.rs` would
/// ever emit (top-level statements have no enclosing declaration at all);
/// this is `frameworks/`'s own bookkeeping, entirely local to this file.
fn ensure_program_main_symbol(
    analysis: &mut FileAnalysis,
    top_level_key: &mut Option<String>,
    span: SourceSpan,
) -> String {
    if let Some(key) = top_level_key {
        return key.clone();
    }
    let key = format!("{}::Program.Main", analysis.file);
    analysis.symbols.push(ExtractedSymbol {
        local_key: key.clone(),
        name: "Main".to_string(),
        qualified_name: key.clone(),
        kind: SymbolKind::Method,
        language: LanguageId::CSharp,
        span,
        parent: None,
        is_exported: false,
        signature_fingerprint: String::new(),
        visibility: Visibility::Public,
        is_partial: false,
        is_record: false,
        partial_ordinal: None,
        roles: Vec::new(),
    });
    *top_level_key = Some(key.clone());
    key
}

// --- call-driven recognition (Q2 PR6, D9) -----------------------------------

/// Dispatches one `invocation_expression` to whichever call-driven pattern
/// (if any) it matches. `source_key` is already resolved to the enclosing
/// method (or the synthetic top-level one) by `walk`.
fn recognize_call(node: Node, source: &[u8], analysis: &mut FileAnalysis, source_key: &str) {
    let Some(info) = call_info(node, source) else {
        return;
    };

    if let Some((_, verb)) = MAP_VERBS.iter().find(|(n, _)| *n == info.name) {
        recognize_map_call(node, source, analysis, source_key, verb, &info);
        return;
    }
    if let Some((_, lifetime)) = ADD_LIFETIMES.iter().find(|(n, _)| *n == info.name) {
        recognize_add_call(node, source, analysis, source_key, lifetime, &info);
        return;
    }
    if TYPED_FEATURE_NAMES.contains(&info.name.as_str()) {
        // Same shape as the `UseMiddleware<T>` branch below: the type
        // argument *is* the registered type. No type argument = a bare
        // framework-service overload, which names no project type — emit
        // nothing rather than guess one.
        if let Some(target) = info.type_args.first() {
            push_heuristic_unresolved(
                analysis,
                source_key,
                RelationshipKind::RegisteredAs,
                target,
                span_of(node),
                Confidence::High,
                Some(format!("key:feature:{}", info.name)),
            );
        }
        return;
    }
    if BARE_FEATURE_ADD_NAMES.contains(&info.name.as_str()) {
        let target = info.name.strip_prefix("Add").unwrap_or(&info.name);
        push_heuristic_unresolved(
            analysis,
            source_key,
            RelationshipKind::RegisteredAs,
            target,
            span_of(node),
            Confidence::High,
            Some(format!("key:feature:{}", info.name)),
        );
        return;
    }
    if info.name == "UseMiddleware" {
        if let Some(target) = info.type_args.first() {
            push_heuristic_unresolved(
                analysis,
                source_key,
                RelationshipKind::RegisteredAs,
                target,
                span_of(node),
                Confidence::High,
                Some("key:middleware".to_string()),
            );
        }
        return;
    }
    if MIDDLEWARE_USE_NAMES.contains(&info.name.as_str()) {
        let target = info.name.strip_prefix("Use").unwrap_or(&info.name);
        push_heuristic_unresolved(
            analysis,
            source_key,
            RelationshipKind::RegisteredAs,
            target,
            span_of(node),
            Confidence::High,
            Some("key:middleware".to_string()),
        );
    }
}

/// Task 6.1/6.2: `Map{Get,Post,Put,Delete,Patch}(<string literal>, <handler>)`
/// — a first argument that isn't a string literal is a partial shape and
/// emits no edge (task 6.8), never a guessed template. `MapGroup("prefix")`
/// chained directly onto the same call (`app.MapGroup("api").MapGet(...)`,
/// i.e. the receiver *is* the `MapGroup` invocation, not a variable holding
/// its result) prefixes the joined path; a prefix held in a variable is the
/// published limitation task 6.2 names, and needs no special-casing here —
/// the receiver there is a plain `identifier`, which `map_group_prefix`
/// simply doesn't match.
fn recognize_map_call(
    node: Node,
    source: &[u8],
    analysis: &mut FileAnalysis,
    source_key: &str,
    verb: &str,
    info: &CallInfo,
) {
    let args = call_arguments(node);
    let Some(first) = args.first() else {
        return;
    };
    let Some(template) = plain_string_literal_text(*first, source) else {
        return;
    };

    let full_path = match info.receiver.and_then(|r| map_group_prefix(r, source)) {
        Some(prefix) => format!(
            "{}/{}",
            prefix.trim_end_matches('/'),
            template.trim_start_matches('/')
        ),
        None => template,
    };
    let target_text = format!("{verb} {full_path}");
    push_handles_route(analysis, source_key, span_of(node), target_text.clone());

    if let Some(handler) = args.get(1) {
        if let Some(handler_name) = identifier_or_method_group_name(*handler, source) {
            push_heuristic_unresolved(
                analysis,
                source_key,
                RelationshipKind::HandlesRoute,
                &handler_name,
                span_of(node),
                Confidence::High,
                Some(format!("route:{target_text}")),
            );
        }
    }
}

fn map_group_prefix(receiver: Node, source: &[u8]) -> Option<String> {
    if receiver.kind() != "invocation_expression" {
        return None;
    }
    let info = call_info(receiver, source)?;
    if info.name != "MapGroup" {
        return None;
    }
    let args = call_arguments(receiver);
    plain_string_literal_text(*args.first()?, source)
}

/// Task 6.3/6.4/D9: `Add{Scoped,Transient,Singleton}` shape ladder — a full
/// `<TService, TImpl>` pair (>=2 type args) is a complete shape (`High`); a
/// single type argument (`Add*<T>()`, self-registration) or the non-generic
/// `AddSingleton(typeof(A), typeof(B))` form are both partial shapes (`Low`,
/// per the Q1 ceiling table's own `.Add*` row: "Name matched but the shape
/// was partial ... -> Low"); a bare name with neither shape (0 type args, no
/// `typeof` pair) matches nothing and emits no edge — never a silent guess.
fn recognize_add_call(
    node: Node,
    source: &[u8],
    analysis: &mut FileAnalysis,
    source_key: &str,
    lifetime: &str,
    info: &CallInfo,
) {
    let span = span_of(node);
    if info.type_args.len() >= 2 {
        push_registered_pair(
            analysis,
            source_key,
            span,
            lifetime,
            &info.type_args[0],
            &info.type_args[1],
            Confidence::High,
        );
        return;
    }
    if info.type_args.len() == 1 {
        push_registered_pair(
            analysis,
            source_key,
            span,
            lifetime,
            &info.type_args[0],
            &info.type_args[0],
            Confidence::Low,
        );
        return;
    }
    let args = call_arguments(node);
    if let (Some(service), Some(impl_name)) = (
        args.first().and_then(|n| typeof_arg_name(*n, source)),
        args.get(1).and_then(|n| typeof_arg_name(*n, source)),
    ) {
        push_registered_pair(
            analysis,
            source_key,
            span,
            lifetime,
            &service,
            &impl_name,
            Confidence::Low,
        );
    }
}

/// The two `RegisteredAs` edges D9's evidence table describes: one from the
/// enclosing method to `Unresolved(impl)` (so a consumer can find what runs),
/// one to `Unresolved(service)` (so a consumer can find the contract) — an
/// edge source must be a symbol, and the service/impl names are, at this
/// point, only names (design "Q2" PR6 table row 3).
fn push_registered_pair(
    analysis: &mut FileAnalysis,
    source_key: &str,
    span: SourceSpan,
    lifetime: &str,
    service: &str,
    impl_name: &str,
    confidence: Confidence,
) {
    push_heuristic_unresolved(
        analysis,
        source_key,
        RelationshipKind::RegisteredAs,
        impl_name,
        span,
        confidence,
        Some(format!("lifetime:{lifetime};role:impl;service:{service}")),
    );
    push_heuristic_unresolved(
        analysis,
        source_key,
        RelationshipKind::RegisteredAs,
        service,
        span,
        confidence,
        Some(format!("lifetime:{lifetime};role:service;impl:{impl_name}")),
    );
}

// --- EF Core DbSet<T> exception (Q3/D11) ------------------------------------

/// Task 6.6/D11: the ONE type-argument-derived edge in the whole system. A
/// class whose `base_list` names (or ends in) `DbContext` gets each of its
/// `property_declaration`/`field_declaration` members checked for a
/// `DbSet<T>`-shaped declared type; every other generic (`List<T>`,
/// `Task<T>`, `IQueryable<T>`, ...) — and every `DbSet<T>` member outside a
/// `DbContext` subclass — is left alone, exactly as `languages/csharp.rs`
/// already does for every other generic.
fn recognize_dbcontext(class: Node, source: &[u8], analysis: &mut FileAnalysis) {
    if !base_list_contains_dbcontext(class, source) {
        return;
    }
    let Some(class_local_key) = find_local_key(analysis, span_of(class), SymbolKind::Class) else {
        return;
    };
    let Some(body) = class.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "property_declaration" => {
                if let (Some(name_node), Some(type_node)) = (
                    member.child_by_field_name("name"),
                    member.child_by_field_name("type"),
                ) {
                    recognize_dbset_member(
                        &class_local_key,
                        name_node,
                        type_node,
                        span_of(member),
                        source,
                        analysis,
                    );
                }
            }
            "field_declaration" => {
                let Some(var_decl) = find_child(member, "variable_declaration") else {
                    continue;
                };
                let Some(type_node) = var_decl.child_by_field_name("type") else {
                    continue;
                };
                let mut dcursor = var_decl.walk();
                for declarator in var_decl.named_children(&mut dcursor) {
                    if declarator.kind() != "variable_declarator" {
                        continue;
                    }
                    let Some(name_node) = declarator.child_by_field_name("name") else {
                        continue;
                    };
                    recognize_dbset_member(
                        &class_local_key,
                        name_node,
                        type_node,
                        span_of(declarator),
                        source,
                        analysis,
                    );
                }
            }
            _ => {}
        }
    }
}

fn base_list_contains_dbcontext(class: Node, source: &[u8]) -> bool {
    let Some(base_list) = find_child(class, "base_list") else {
        return false;
    };
    let mut cursor = base_list.walk();
    let result = base_list.named_children(&mut cursor).any(|entry| {
        let type_node = match entry.kind() {
            "primary_constructor_base_type" => entry.child_by_field_name("type"),
            "argument_list" => None,
            _ => Some(entry),
        };
        type_node
            .and_then(|t| simple_type_text(t, source))
            .map(|name| name == "DbContext" || name.ends_with("DbContext"))
            .unwrap_or(false)
    });
    result
}

/// One `PersistsTo` edge iff `type_node` is exactly `generic_name DbSet<T>`
/// with a single type argument — `List<T>`/`Task<T>`/`IQueryable<T>`/a
/// non-generic type/a 2+-argument generic (`Dictionary<A, B>`) all fall
/// through to nothing, per D11's exact bound.
fn recognize_dbset_member(
    class_local_key: &str,
    name_node: Node,
    type_node: Node,
    span: SourceSpan,
    source: &[u8],
    analysis: &mut FileAnalysis,
) {
    if type_node.kind() != "generic_name" {
        return;
    }
    let Some((type_name, type_args)) = generic_name_parts(type_node, source) else {
        return;
    };
    if type_name != "DbSet" || type_args.len() != 1 {
        return;
    }
    let Ok(prop_name) = name_node.utf8_text(source) else {
        return;
    };
    push_heuristic_unresolved(
        analysis,
        class_local_key,
        RelationshipKind::PersistsTo,
        &type_args[0],
        span,
        Confidence::High,
        Some(format!("dbset:{prop_name}")),
    );
}

// --- shared call/type parsing helpers ---------------------------------------

/// An `invocation_expression`'s callee name, type-argument list (if any),
/// and receiver expression (if any) — `Foo()` (bare, no receiver),
/// `Foo<T>()` (bare, generic), `obj.Foo()`/`obj.Foo<T>()` (member access,
/// receiver = `obj`).
fn call_info<'a>(invocation: Node<'a>, source: &[u8]) -> Option<CallInfo<'a>> {
    let func = invocation.child_by_field_name("function")?;
    match func.kind() {
        "identifier" => Some(CallInfo {
            name: func.utf8_text(source).ok()?.to_string(),
            type_args: Vec::new(),
            receiver: None,
        }),
        "generic_name" => {
            let (name, type_args) = generic_name_parts(func, source)?;
            Some(CallInfo {
                name,
                type_args,
                receiver: None,
            })
        }
        "member_access_expression" => {
            let name_node = func.child_by_field_name("name")?;
            let receiver = func.child_by_field_name("expression");
            match name_node.kind() {
                "identifier" => Some(CallInfo {
                    name: name_node.utf8_text(source).ok()?.to_string(),
                    type_args: Vec::new(),
                    receiver,
                }),
                "generic_name" => {
                    let (name, type_args) = generic_name_parts(name_node, source)?;
                    Some(CallInfo {
                        name,
                        type_args,
                        receiver,
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// A `generic_name`'s own identifier and its `type_argument_list`'s entries,
/// each resolved to a simple name by `simple_type_text` (D1: this file's own
/// re-parse, not `cs_simple_type_name`, since that one is `languages/`-only
/// and this PR's whole point is recovering what it drops).
fn generic_name_parts(node: Node, source: &[u8]) -> Option<(String, Vec<String>)> {
    let mut cursor = node.walk();
    let ident = node
        .named_children(&mut cursor)
        .find(|c| c.kind() == "identifier")?;
    let name = ident.utf8_text(source).ok()?.to_string();
    let mut lcursor = node.walk();
    let type_args = node
        .named_children(&mut lcursor)
        .find(|c| c.kind() == "type_argument_list")
        .map(|list| {
            let mut acursor = list.walk();
            list.named_children(&mut acursor)
                .filter_map(|t| simple_type_text(t, source))
                .collect()
        })
        .unwrap_or_default();
    Some((name, type_args))
}

/// Same simple-(last-segment)-name resolution `languages/csharp.rs`'s
/// `cs_simple_type_name` uses, duplicated here (D1's own rationale: this
/// module never reads `languages/`'s private helpers).
fn simple_type_text(node: Node, source: &[u8]) -> Option<String> {
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

/// An `invocation_expression`'s positional argument expressions, in source
/// order — each `argument`'s own expression child, unwrapped exactly like
/// `attribute_string_args` unwraps an `attribute_argument`.
fn call_arguments(invocation: Node) -> Vec<Node> {
    let Some(arg_list) = invocation.child_by_field_name("arguments") else {
        return Vec::new();
    };
    let mut cursor = arg_list.walk();
    arg_list
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "argument")
        .filter_map(|arg| {
            let mut acursor = arg.walk();
            arg.named_children(&mut acursor).last()
        })
        .collect()
}

fn plain_string_literal_text(node: Node, source: &[u8]) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let text = node.utf8_text(source).ok()?;
    Some(text.trim_matches('"').to_string())
}

/// Task 6.1: a `Map*` handler argument that names a project symbol — a bare
/// identifier (`GetInvoice`) or a method group's last segment
/// (`InvoiceHandlers.GetInvoice`). An inline lambda (`(req) => ...`) names
/// nothing and resolves to `None` here, never a guess.
fn identifier_or_method_group_name(node: Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(source).ok().map(str::to_string),
        "member_access_expression" => node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok())
            .map(str::to_string),
        _ => None,
    }
}

fn typeof_arg_name(node: Node, source: &[u8]) -> Option<String> {
    if node.kind() != "typeof_expression" {
        return None;
    }
    let ty = node.child_by_field_name("type")?;
    simple_type_text(ty, source)
}

fn push_heuristic_unresolved(
    analysis: &mut FileAnalysis,
    source_key: &str,
    kind: RelationshipKind,
    target: &str,
    span: SourceSpan,
    confidence: Confidence,
    reason: Option<String>,
) {
    analysis.relationships.push(ExtractedRelationship {
        source_local_key: source_key.to_string(),
        target: EdgeTarget::Unresolved(target.to_string()),
        kind,
        span,
        provenance: Provenance::Heuristic,
        confidence,
        reason,
    });
}

/// design "Q2" table rows 1-2: `[ApiController]`/`*Controller`-named role
/// tagging, `[Route("tpl")]` class-level prefix, per-method route walking.
fn recognize_class(class: Node, source: &[u8], analysis: &mut FileAnalysis) {
    let Some(name_node) = class.child_by_field_name("name") else {
        return;
    };
    let Ok(name) = name_node.utf8_text(source) else {
        return;
    };
    let Some(class_local_key) = find_local_key(analysis, span_of(class), SymbolKind::Class) else {
        return;
    };

    let class_attrs = attribute_lists_of(class, source);
    let has_api_controller = class_attrs.iter().any(|a| {
        AttrPattern {
            name: "ApiController",
        }
        .matches(&a.name)
    });
    let class_route_prefix = class_attrs
        .iter()
        .find(|a| AttrPattern { name: "Route" }.matches(&a.name))
        .and_then(|a| a.args.first().cloned());
    let name_ends_controller = name.ends_with("Controller");

    let mut has_route_member = false;
    if let Some(body) = class.child_by_field_name("body") {
        let mut cursor = body.walk();
        for member in body.named_children(&mut cursor) {
            if member.kind() != "method_declaration" {
                continue;
            }
            if recognize_method(member, source, analysis, class_route_prefix.as_deref()) {
                has_route_member = true;
            }
        }
    }

    // Task 5.1/5.6: `[ApiController]` alone is enough; a `*Controller`-named
    // class needs at least one `[Route]`/`[Http*]` member to earn the role
    // (a `*Controller`-named class with no routing attribute at all is not
    // evidence, just a name).
    if has_api_controller || (name_ends_controller && has_route_member) {
        add_role(analysis, &class_local_key, FrameworkRole::Controller);
    }
}

/// design "Q2" table rows 3-5: HTTP verb attributes -> `HandlesRoute` +
/// `Route` role; `[Function("Name")]` -> `Route` role only; a trigger
/// attribute (on the method or one of its parameters) -> `Triggers`.
/// Returns whether the method carried an `[Http*]`/`[Route]` attribute, so
/// the caller can decide `*Controller`-name role tagging (task 5.6).
fn recognize_method(
    method: Node,
    source: &[u8],
    analysis: &mut FileAnalysis,
    class_prefix: Option<&str>,
) -> bool {
    let Some(method_local_key) = find_local_key(analysis, span_of(method), SymbolKind::Method)
    else {
        return false;
    };
    let method_attrs = attribute_lists_of(method, source);
    let mut had_route_attr = false;

    for attr in &method_attrs {
        if let Some((_, verb)) = HTTP_VERB_ATTRS.iter().find(|(n, _)| *n == attr.name) {
            had_route_attr = true;
            add_role(analysis, &method_local_key, FrameworkRole::Route);
            let template = join_template(class_prefix, attr.args.first().map(String::as_str));
            let target_text = format!("{verb} {template}").trim_end().to_string();
            push_handles_route(analysis, &method_local_key, span_of(method), target_text);
        }
        if (AttrPattern { name: "Function" }).matches(&attr.name) {
            had_route_attr = true;
            add_role(analysis, &method_local_key, FrameworkRole::Route);
        }
    }

    for attr in trigger_attrs(method, &method_attrs, source) {
        push_triggers(analysis, &method_local_key, span_of(method), &attr);
    }

    had_route_attr
}

/// design "Q2" table row 6: any attribute name ending in `Trigger`, whether
/// it sits on the method itself or on one of its parameters — the trigger
/// source is always the enclosing method, not the parameter.
fn trigger_attrs(method: Node, method_attrs: &[Attr], source: &[u8]) -> Vec<Attr> {
    let mut out: Vec<Attr> = method_attrs
        .iter()
        .filter(|a| a.name.ends_with("Trigger"))
        .cloned()
        .collect();

    if let Some(params) = method.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            if param.kind() != "parameter" {
                continue;
            }
            out.extend(
                attribute_lists_of(param, source)
                    .into_iter()
                    .filter(|a| a.name.ends_with("Trigger")),
            );
        }
    }
    out
}

/// design "Q2" row 2: class `[Route]` prefix joined with the method's own
/// template — a plain `/`-normalized join, neither half required.
fn join_template(prefix: Option<&str>, method_tpl: Option<&str>) -> String {
    match (prefix, method_tpl.filter(|t| !t.is_empty())) {
        (Some(p), Some(m)) => format!("{}/{}", p.trim_end_matches('/'), m.trim_start_matches('/')),
        (Some(p), None) => p.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => String::new(),
    }
}

fn push_handles_route(
    analysis: &mut FileAnalysis,
    source_local_key: &str,
    span: SourceSpan,
    target_text: String,
) {
    analysis.relationships.push(ExtractedRelationship {
        source_local_key: source_local_key.to_string(),
        // Task 5.8: a route template is never a project symbol — stored as
        // `target_external` directly, not `Unresolved` (which would send it
        // through `resolve.rs`'s by-name project lookup and fail as
        // `no matching symbol in project`).
        target: EdgeTarget::External(target_text.clone()),
        kind: RelationshipKind::HandlesRoute,
        span,
        provenance: Provenance::Heuristic,
        confidence: Confidence::High,
        reason: Some(format!("route:{target_text}")),
    });
}

fn push_triggers(
    analysis: &mut FileAnalysis,
    source_local_key: &str,
    span: SourceSpan,
    attr: &Attr,
) {
    let first_arg = attr.args.first().cloned().unwrap_or_default();
    analysis.relationships.push(ExtractedRelationship {
        source_local_key: source_local_key.to_string(),
        target: EdgeTarget::External(attr.name.clone()),
        kind: RelationshipKind::Triggers,
        span,
        provenance: Provenance::Heuristic,
        confidence: Confidence::High,
        reason: Some(format!("trigger:{first_arg}")),
    });
}

fn find_local_key(analysis: &FileAnalysis, span: SourceSpan, kind: SymbolKind) -> Option<String> {
    analysis
        .symbols
        .iter()
        .find(|s| s.kind == kind && s.span == span)
        .map(|s| s.local_key.clone())
}

fn add_role(analysis: &mut FileAnalysis, local_key: &str, role: FrameworkRole) {
    if let Some(sym) = analysis
        .symbols
        .iter_mut()
        .find(|s| s.local_key == local_key)
    {
        if !sym.roles.contains(&role) {
            sym.roles.push(role);
        }
    }
}

/// Every `attribute` inside every `attribute_list` child of `node` (the
/// same shape `languages/csharp.rs`'s `collect_attributes` walks for
/// `Decorates`), plus each attribute's string-literal arguments in source
/// order.
fn attribute_lists_of(node: Node, source: &[u8]) -> Vec<Attr> {
    let mut cursor = node.walk();
    let mut out = Vec::new();
    for list in node
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "attribute_list")
    {
        let mut inner = list.walk();
        for attribute in list.named_children(&mut inner) {
            if attribute.kind() != "attribute" {
                continue;
            }
            let Some(name_node) = attribute.child_by_field_name("name") else {
                continue;
            };
            let Ok(name) = name_node.utf8_text(source) else {
                continue;
            };
            out.push(Attr {
                name: name.to_string(),
                args: attribute_string_args(attribute, source),
            });
        }
    }
    out
}

/// An `attribute`'s `attribute_argument_list` -> each `attribute_argument`'s
/// string-literal text, quotes stripped. A non-string-literal argument
/// (an enum member, a named argument's identifier value, ...) is skipped,
/// not guessed — this PR's evidence only ever needs the literal route
/// template / function name / trigger argument.
fn attribute_string_args(attribute: Node, source: &[u8]) -> Vec<String> {
    let mut cursor = attribute.walk();
    let Some(arg_list) = attribute
        .named_children(&mut cursor)
        .find(|c| c.kind() == "attribute_argument_list")
    else {
        return Vec::new();
    };
    let mut acursor = arg_list.walk();
    arg_list
        .named_children(&mut acursor)
        .filter(|c| c.kind() == "attribute_argument")
        .filter_map(|arg| string_literal_text(arg, source))
        .collect()
}

fn string_literal_text(attribute_argument: Node, source: &[u8]) -> Option<String> {
    let mut cursor = attribute_argument.walk();
    let expr = attribute_argument.named_children(&mut cursor).last()?;
    if expr.kind() != "string_literal" {
        return None;
    }
    let text = expr.utf8_text(source).ok()?;
    Some(text.trim_matches('"').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;
    use codekurve_core::LanguageId;

    fn analyze(source: &str) -> FileAnalysis {
        extract::analyze(
            source,
            LanguageId::CSharp,
            "src/Controllers/InvoiceController.cs",
        )
        .unwrap()
    }

    fn roles_of<'a>(analysis: &'a FileAnalysis, name: &str) -> &'a [FrameworkRole] {
        &analysis
            .symbols
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("no symbol named {name}"))
            .roles
    }

    // --- task 5.5 ------------------------------------------------------

    #[test]
    fn api_controller_route_and_httpget_join_class_and_method_templates() {
        let source = r#"
[ApiController]
[Route("api/invoices")]
public class InvoiceController : ControllerBase
{
    [HttpGet("{id}")]
    public IActionResult GetById(int id) => Ok();
}
"#;
        let analysis = analyze(source);

        assert_eq!(
            roles_of(&analysis, "InvoiceController"),
            &[FrameworkRole::Controller]
        );
        assert_eq!(roles_of(&analysis, "GetById"), &[FrameworkRole::Route]);

        let route = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::HandlesRoute)
            .expect("expected a HandlesRoute edge");
        assert_eq!(
            route.target,
            EdgeTarget::External("GET api/invoices/{id}".to_string())
        );
        assert_eq!(route.reason.as_deref(), Some("route:GET api/invoices/{id}"));
        assert_eq!(route.provenance, Provenance::Heuristic);
    }

    // --- task 5.6 ------------------------------------------------------

    #[test]
    fn controller_named_class_without_api_controller_still_tagged_by_http_member() {
        let source = r#"
public class InvoiceController
{
    [HttpGet("api/invoices")]
    public IActionResult List() => Ok();
}
"#;
        let analysis = analyze(source);
        assert_eq!(
            roles_of(&analysis, "InvoiceController"),
            &[FrameworkRole::Controller]
        );
    }

    #[test]
    fn controller_named_class_with_no_routing_attribute_gets_no_role() {
        let source = r#"
public class InvoiceController
{
    public void DoWork() {}
}
"#;
        let analysis = analyze(source);
        assert!(roles_of(&analysis, "InvoiceController").is_empty());
    }

    // --- task 5.7 ------------------------------------------------------

    #[test]
    fn azure_function_and_each_trigger_attribute_produce_triggers_edges() {
        let source = r#"
public class InvoiceFunctions
{
    [Function("GetInvoice")]
    public IActionResult Run(
        [HttpTrigger(AuthorizationLevel.Function, "get", Route = "invoices/{id}")] HttpRequest req,
        [QueueTrigger("invoices-queue")] string message)
    {
        return null;
    }
}
"#;
        let analysis = analyze(source);
        assert_eq!(roles_of(&analysis, "Run"), &[FrameworkRole::Route]);

        let triggers: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Triggers)
            .collect();
        assert_eq!(triggers.len(), 2);

        let http_trigger = triggers
            .iter()
            .find(|r| r.target == EdgeTarget::External("HttpTrigger".to_string()))
            .expect("expected an HttpTrigger edge");
        // First *string-literal* argument in source order — `AuthorizationLevel.Function` is
        // not a string literal so it's skipped, leaving `"get"` as the first one.
        assert_eq!(http_trigger.reason.as_deref(), Some("trigger:get"));

        let queue_trigger = triggers
            .iter()
            .find(|r| r.target == EdgeTarget::External("QueueTrigger".to_string()))
            .expect("expected a QueueTrigger edge");
        assert_eq!(
            queue_trigger.reason.as_deref(),
            Some("trigger:invoices-queue")
        );

        for edge in &triggers {
            assert_eq!(edge.provenance, Provenance::Heuristic);
        }
    }

    // --- task 5.8 --------------------------------------------------------

    #[test]
    fn route_target_is_external_not_a_synthetic_symbol() {
        let source = r#"
[ApiController]
public class InvoiceController
{
    [HttpGet("api/invoices")]
    public IActionResult List() => Ok();
}
"#;
        let analysis = analyze(source);
        let route = analysis
            .relationships
            .iter()
            .find(|r| r.kind == RelationshipKind::HandlesRoute)
            .unwrap();
        assert!(matches!(route.target, EdgeTarget::External(_)));
    }

    // --- task 5.9 --------------------------------------------------------

    #[test]
    fn every_dotnet_attribute_driven_edge_is_heuristic_never_extracted_or_resolved() {
        let source = r#"
[ApiController]
[Route("api/invoices")]
public class InvoiceController : ControllerBase
{
    [HttpGet("{id}")]
    public IActionResult GetById(int id) => Ok();
}

public class InvoiceFunctions
{
    [Function("GetInvoice")]
    public IActionResult Run([HttpTrigger(AuthorizationLevel.Function, "get")] HttpRequest req)
    {
        return null;
    }
}
"#;
        let analysis = analyze(source);
        let framework_edges: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| {
                matches!(
                    r.kind,
                    RelationshipKind::HandlesRoute | RelationshipKind::Triggers
                )
            })
            .collect();
        assert!(!framework_edges.is_empty());
        for edge in framework_edges {
            assert_eq!(edge.provenance, Provenance::Heuristic);
            assert_ne!(edge.provenance, Provenance::Extracted);
            assert_ne!(edge.provenance, Provenance::Resolved);
        }
    }

    // ==================== PR6: call-driven + EF Core ====================

    fn analyze_program(source: &str) -> FileAnalysis {
        extract::analyze(source, LanguageId::CSharp, "src/Program.cs").unwrap()
    }

    fn call_driven_rel(
        analysis: &FileAnalysis,
        kind: RelationshipKind,
    ) -> Vec<&ExtractedRelationship> {
        analysis
            .relationships
            .iter()
            .filter(|r| r.kind == kind)
            .collect()
    }

    // --- task 6.8 ----------------------------------------------------------

    #[test]
    fn map_get_with_literal_template_and_handler_produces_two_handles_route_edges() {
        let source = r#"
var app = WebApplication.Create();
app.MapGet("/invoices/{id}", GetInvoice);
"#;
        let analysis = analyze_program(source);
        let routes = call_driven_rel(&analysis, RelationshipKind::HandlesRoute);
        assert_eq!(routes.len(), 2);

        let template_edge = routes
            .iter()
            .find(|r| matches!(&r.target, EdgeTarget::External(t) if t == "GET /invoices/{id}"))
            .expect("expected the External route-template edge");
        assert_eq!(
            template_edge.reason.as_deref(),
            Some("route:GET /invoices/{id}")
        );
        assert_eq!(template_edge.confidence, Confidence::High);

        let handler_edge = routes
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("GetInvoice".to_string()))
            .expect("expected the Unresolved handler edge");
        assert_eq!(
            handler_edge.reason.as_deref(),
            Some("route:GET /invoices/{id}")
        );
    }

    #[test]
    fn map_get_without_a_literal_first_argument_produces_no_edge() {
        let source = r#"
var app = WebApplication.Create();
string template = "/invoices/{id}";
app.MapGet(template, GetInvoice);
"#;
        let analysis = analyze_program(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::HandlesRoute).is_empty());
    }

    #[test]
    fn map_group_chained_on_the_same_statement_prefixes_the_joined_path() {
        let source = r#"
var app = WebApplication.Create();
app.MapGroup("api").MapGet("/users", GetUsers);
"#;
        let analysis = analyze_program(source);
        let routes = call_driven_rel(&analysis, RelationshipKind::HandlesRoute);
        let template_edge = routes
            .iter()
            .find(|r| matches!(&r.target, EdgeTarget::External(_)))
            .expect("expected the External route-template edge");
        assert_eq!(
            template_edge.target,
            EdgeTarget::External("GET api/users".to_string())
        );
    }

    #[test]
    fn map_group_held_in_a_variable_is_the_published_limitation_no_prefix() {
        let source = r#"
var app = WebApplication.Create();
var api = app.MapGroup("api");
api.MapGet("/users", GetUsers);
"#;
        let analysis = analyze_program(source);
        let routes = call_driven_rel(&analysis, RelationshipKind::HandlesRoute);
        let template_edge = routes
            .iter()
            .find(|r| matches!(&r.target, EdgeTarget::External(_)))
            .expect("expected the External route-template edge");
        // No prefix applied — `api` is a plain identifier receiver, not the
        // `MapGroup(...)` invocation itself (task 6.2's published limitation).
        assert_eq!(
            template_edge.target,
            EdgeTarget::External("GET /users".to_string())
        );
    }

    // --- task 6.9 ------------------------------------------------------------

    #[test]
    fn add_scoped_with_two_type_arguments_produces_paired_registered_as_edges_at_high() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddScoped<IInvoiceRepository, InvoiceRepository>();
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 2);

        let impl_edge = regs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("InvoiceRepository".to_string()))
            .expect("expected the impl edge");
        assert_eq!(
            impl_edge.reason.as_deref(),
            Some("lifetime:scoped;role:impl;service:IInvoiceRepository")
        );
        assert_eq!(impl_edge.confidence, Confidence::High);

        let service_edge = regs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("IInvoiceRepository".to_string()))
            .expect("expected the service edge");
        assert_eq!(
            service_edge.reason.as_deref(),
            Some("lifetime:scoped;role:service;impl:InvoiceRepository")
        );
        assert_eq!(service_edge.confidence, Confidence::High);
    }

    #[test]
    fn add_scoped_with_one_type_argument_produces_the_same_pair_at_low() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddScoped<InvoiceService>();
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 2);
        for edge in regs {
            assert_eq!(
                edge.target,
                EdgeTarget::Unresolved("InvoiceService".to_string())
            );
            assert_eq!(edge.confidence, Confidence::Low);
        }
    }

    #[test]
    fn add_singleton_typeof_pair_produces_the_same_pair_at_low() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddSingleton(typeof(IFoo), typeof(Foo));
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 2);
        for edge in &regs {
            assert_eq!(edge.confidence, Confidence::Low);
        }
        assert!(regs
            .iter()
            .any(|r| r.target == EdgeTarget::Unresolved("Foo".to_string())));
        assert!(regs
            .iter()
            .any(|r| r.target == EdgeTarget::Unresolved("IFoo".to_string())));
    }

    #[test]
    fn add_with_zero_type_arguments_and_no_typeof_shape_produces_no_edge() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddScoped();
"#;
        let analysis = analyze_program(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::RegisteredAs).is_empty());
    }

    // --- middleware (task 6.5) ------------------------------------------------

    #[test]
    fn use_middleware_generic_and_closed_list_use_call_produce_registered_as() {
        let source = r#"
var app = WebApplication.Create();
app.UseMiddleware<RequestLoggingMiddleware>();
app.UseCors();
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 2);
        assert!(regs.iter().any(|r| r.target
            == EdgeTarget::Unresolved("RequestLoggingMiddleware".to_string())
            && r.reason.as_deref() == Some("key:middleware")));
        assert!(regs
            .iter()
            .any(|r| r.target == EdgeTarget::Unresolved("Cors".to_string())
                && r.reason.as_deref() == Some("key:middleware")));
    }

    // --- typed / bare feature registrations ----------------------------------

    #[test]
    fn typed_feature_add_call_registers_its_type_argument() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddDbContextFactory<TodoDbContext>();
builder.Services.AddHostedService<CleanupWorker>();
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 2);

        let db = regs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("TodoDbContext".to_string()))
            .expect("expected the AddDbContextFactory edge");
        assert_eq!(
            db.reason.as_deref(),
            Some("key:feature:AddDbContextFactory")
        );
        assert_eq!(db.confidence, Confidence::High);
        assert_eq!(db.provenance, Provenance::Heuristic);

        let worker = regs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("CleanupWorker".to_string()))
            .expect("expected the AddHostedService edge");
        assert_eq!(
            worker.reason.as_deref(),
            Some("key:feature:AddHostedService")
        );
        assert_eq!(worker.confidence, Confidence::High);
    }

    /// The classic (pre-minimal-API) host builder, still in production. Taken
    /// from a real 350-file ASP.NET Core app: `UseStartup<Startup>` names a
    /// project class exactly like the `Add*` typed registrations do, and
    /// `UseMvc`/`AddMvcCore` are that app's pipeline, invisible to a catalogue
    /// surveyed only from minimal-API code.
    #[test]
    fn classic_host_builder_registrations_are_recognized() {
        let source = r#"
public class Program {
    public static void Main(string[] args) {
        WebHost.CreateDefaultBuilder(args).UseStartup<Startup>().Build().Run();
    }
}
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        let startup = regs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("Startup".to_string()))
            .expect("expected the UseStartup edge");
        assert_eq!(startup.reason.as_deref(), Some("key:feature:UseStartup"));
        assert_eq!(startup.confidence, Confidence::High);
        assert_eq!(startup.provenance, Provenance::Heuristic);

        // `UseMvc`/`AddMvcCore` are middleware/bare-feature shapes: recognized,
        // but their target is a framework feature, not a project type.
        let classic = analyze_program(
            r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddMvcCore();
app.UseMvc();
"#,
        );
        let names: Vec<_> = call_driven_rel(&classic, RelationshipKind::RegisteredAs)
            .iter()
            .map(|r| r.target.clone())
            .collect();
        assert!(names.contains(&EdgeTarget::Unresolved("MvcCore".to_string())));
        assert!(names.contains(&EdgeTarget::Unresolved("Mvc".to_string())));
    }

    #[test]
    fn typed_feature_add_call_without_a_type_argument_produces_no_edge() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddHttpClient();
builder.Services.AddDbContext(options => options.UseSqlServer(cs));
"#;
        let analysis = analyze_program(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::RegisteredAs).is_empty());
    }

    #[test]
    fn bare_feature_add_call_registers_the_feature_name() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddOpenApi();
builder.Services.AddProblemDetails();
"#;
        let analysis = analyze_program(source);
        let regs = call_driven_rel(&analysis, RelationshipKind::RegisteredAs);
        assert_eq!(regs.len(), 2);

        let open_api = regs
            .iter()
            .find(|r| r.target == EdgeTarget::Unresolved("OpenApi".to_string()))
            .expect("expected the AddOpenApi edge");
        assert_eq!(open_api.reason.as_deref(), Some("key:feature:AddOpenApi"));
        assert_eq!(open_api.confidence, Confidence::High);
        assert_eq!(open_api.provenance, Provenance::Heuristic);

        assert!(regs.iter().any(|r| r.target
            == EdgeTarget::Unresolved("ProblemDetails".to_string())
            && r.reason.as_deref() == Some("key:feature:AddProblemDetails")));
    }

    /// D9's exact-name rule, proven negatively: EF migration/model-builder
    /// calls and `DateTime` arithmetic all match a naive `.Add*(`/`.Use*(`
    /// regex but are not registrations, so none of them is in either closed
    /// list and none produces an edge.
    #[test]
    fn non_registration_add_and_use_calls_produce_no_edge() {
        let source = r#"
public class Migration
{
    public void Up(MigrationBuilder migrationBuilder, ModelBuilder modelBuilder)
    {
        migrationBuilder.AddColumn<string>("Title", "Todos");
        migrationBuilder.AddForeignKey("FK_Todo_User", "Todos", "UserId");
        modelBuilder.Entity<Todo>().Property(t => t.Id).UseIdentityColumn();
        modelBuilder.HasAnnotation("Relational:Collation", "x").AddAnnotation("a", "b");
        var later = DateTime.UtcNow.AddTicks(5);
        options.UseInMemoryDatabase("todos");
        options.UseSqlServer(connectionString);
        options.UseNpgsql(connectionString);
        options.UseSqlite(connectionString);
        options.AddPolicy("default", policy => policy.AllowAnyOrigin());
    }
}
"#;
        let analysis = analyze(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::RegisteredAs).is_empty());
    }

    // --- task 6.11 -------------------------------------------------------------

    #[test]
    fn add_something_custom_not_in_the_closed_list_produces_no_edge() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddSomethingCustom<IFoo, Foo>();
"#;
        let analysis = analyze_program(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::RegisteredAs).is_empty());
    }

    #[test]
    fn use_something_custom_not_in_the_closed_list_produces_no_edge() {
        let source = r#"
var app = WebApplication.Create();
app.UseSomethingCustom();
"#;
        let analysis = analyze_program(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::RegisteredAs).is_empty());
    }

    // --- task 6.10: EF Core bound ------------------------------------------

    #[test]
    fn dbset_in_a_dbcontext_subclass_produces_one_persists_to_edge() {
        let source = r#"
public class AppDbContext : DbContext
{
    public DbSet<Invoice> Invoices { get; set; }
}
"#;
        let analysis = analyze(source);
        let edges = call_driven_rel(&analysis, RelationshipKind::PersistsTo);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].target,
            EdgeTarget::Unresolved("Invoice".to_string())
        );
        assert_eq!(edges[0].reason.as_deref(), Some("dbset:Invoices"));
        assert_eq!(edges[0].confidence, Confidence::High);
    }

    #[test]
    fn dbset_outside_a_dbcontext_subclass_produces_no_edge() {
        let source = r#"
public class InvoiceRepository
{
    public DbSet<Invoice> Invoices { get; set; }
}
"#;
        let analysis = analyze(source);
        assert!(call_driven_rel(&analysis, RelationshipKind::PersistsTo).is_empty());
    }

    #[test]
    fn non_dbset_generics_in_a_dbcontext_subclass_produce_no_edge() {
        let source = r#"
public class AppDbContext : DbContext
{
    public DbSet<Invoice> Invoices { get; set; }
    public List<Invoice> CachedInvoices { get; set; }
    public Task<Invoice> PendingInvoice { get; set; }
    public IQueryable<Invoice> QueryableInvoices { get; set; }
    public Dictionary<string, Invoice> InvoicesById { get; set; }
}
"#;
        let analysis = analyze(source);
        let edges = call_driven_rel(&analysis, RelationshipKind::PersistsTo);
        // Exactly the one `DbSet<Invoice>` member — every other generic
        // shape (`List<T>`, `Task<T>`, `IQueryable<T>`, `Dictionary<A,B>`)
        // is left alone, per D11's exact bound.
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].reason.as_deref(), Some("dbset:Invoices"));
    }

    // --- task 6.12 -------------------------------------------------------------

    #[test]
    fn every_dotnet_call_driven_and_efcore_edge_is_heuristic_never_extracted_or_resolved() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddScoped<IInvoiceRepository, InvoiceRepository>();
builder.Services.AddDbContext<AppDbContext>();
builder.Services.AddOpenApi();
var app = builder.Build();
app.UseMiddleware<RequestLoggingMiddleware>();
app.MapGet("/invoices/{id}", GetInvoice);

public class AppDbContext : DbContext
{
    public DbSet<Invoice> Invoices { get; set; }
}
"#;
        let analysis = analyze_program(source);
        let framework_edges: Vec<&ExtractedRelationship> = analysis
            .relationships
            .iter()
            .filter(|r| {
                matches!(
                    r.kind,
                    RelationshipKind::HandlesRoute
                        | RelationshipKind::RegisteredAs
                        | RelationshipKind::PersistsTo
                )
            })
            .collect();
        assert!(!framework_edges.is_empty());
        for edge in framework_edges {
            assert_eq!(edge.provenance, Provenance::Heuristic);
            assert_ne!(edge.provenance, Provenance::Extracted);
            assert_ne!(edge.provenance, Provenance::Resolved);
        }
    }

    // --- task 6.14: runtime harness ---------------------------------------

    #[test]
    fn runtime_harness_program_cs_map_add_dbset_trio() {
        let source = r#"
var builder = WebApplication.CreateBuilder(args);
builder.Services.AddScoped<IInvoiceRepository, InvoiceRepository>();
var app = builder.Build();
app.MapGet("/invoices/{id}", GetInvoice);

public class AppDbContext : DbContext
{
    public DbSet<Invoice> Invoices { get; set; }
}
"#;
        let analysis = analyze_program(source);
        assert!(!call_driven_rel(&analysis, RelationshipKind::RegisteredAs).is_empty());
        assert!(!call_driven_rel(&analysis, RelationshipKind::HandlesRoute).is_empty());
        assert!(!call_driven_rel(&analysis, RelationshipKind::PersistsTo).is_empty());
    }
}
