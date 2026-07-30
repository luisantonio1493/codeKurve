```yaml
schema: gentle-ai.verify-result/v1
evidence_revision: sha256:79153f8c177d96ed428b9a28a7622a0be6bdfd84327b061af73549736b27da41
verdict: fail
blockers: 1
critical_findings: 3
requirements: 15/21
scenarios: 35/46
test_command: cargo test --workspace
test_exit_code: 0
test_output_hash: sha256:e709bf53fe404345a3f303c9bf84eb8339d2ac4268897c9297a2328d3c612f6b
build_command: cargo build --workspace
build_exit_code: 0
build_output_hash: sha256:732186322402325172940531d8d6d7cfac85dc902585b2bef6715ad6839dacfd
```

## Verification Report

**Change**: phase-5-csharp
**Version**: N/A
**Mode**: Standard

### Completeness
| Metric | Value |
|--------|-------|
| Tasks total | 84 |
| Tasks complete | 84 |
| Tasks incomplete | 0 |

### Build & Tests Execution
**Build**: ✅ Passed
```text
cargo build --workspace
exit 0
```

**Tests**: ✅ 134 passed / ❌ 0 failed / ⚠️ 0 skipped
```text
cargo test --workspace
exit 0
```

**Coverage**: ➖ Not available

### Spec Compliance Matrix
| Requirement | Scenario | Test | Result |
|-------------|----------|------|--------|
| C# Symbol Extraction | File-scoped namespace | csharp.rs > file_scoped_namespace | ✅ COMPLIANT |
| C# Symbol Extraction | Block-scoped namespace with nested class | csharp.rs > block_namespace_with_nested_class | ✅ COMPLIANT |
| C# Symbol Extraction | Enum members index as Field | csharp.rs > enum_members_index_as_field | ✅ COMPLIANT |
| C# Symbol Extraction | Constructor, method, property, field | csharp.rs > constructor_method_property_field_all_indexed | ✅ COMPLIANT |
| using Directives | Plain using resolves to in-project namespace | csharp_graph_fixture.rs > csharp_graph_fixture_resolves_cross_file_relationships | ✅ COMPLIANT |
| using Directives | using static recorded without resolving member visibility | (none found for call-side unresolved behavior) | ❌ UNTESTED |
| using Directives | using alias recorded, alias-qualified ref unresolved | csharp.rs > using_directive_forms_produce_imports_with_reason (directive only) | ❌ UNTESTED |
| Base List Pending | Multiple base-list entries each own pending reference | csharp.rs > base_list_emits_one_pending_reference_per_entry | ✅ COMPLIANT |
| Calls/Constructs | Direct call produces Calls | csharp.rs > calls_constructs_and_target_typed_new | ✅ COMPLIANT |
| Calls/Constructs | Object creation produces Constructs | csharp.rs > calls_constructs_and_target_typed_new | ✅ COMPLIANT |
| Calls/Constructs | Target-typed new() unresolved | csharp.rs > calls_constructs_and_target_typed_new | ✅ COMPLIANT |
| Attributes Decorates | Attribute on class with own span | csharp.rs > attributes_produce_decorates_with_own_span | ✅ COMPLIANT |
| Attributes Decorates | No attribute name special-cased | csharp.rs > attributes_produce_decorates_with_own_span | ✅ COMPLIANT |
| Visibility Round-Trip | All six levels distinct after indexing/query | (store/extract covered; CLI symbol omits visibility) | ❌ UNTESTED |
| Generics Structural | Generic class where constraint fingerprint, no edges | csharp.rs > generic_constraint_recorded_in_fingerprint_no_edge | ✅ COMPLIANT |
| Generics Structural | Type argument at instantiation does not resolve | (none found) | ❌ UNTESTED |
| Partial Not Merged | Partial fragments in different files independent | partial_identity.rs > partial_fragments_keep_distinct_identity_while_non_partial_stays_unset | ⚠️ PARTIAL |
| Partial Not Merged | Reference to partial type is ambiguity set | resolve.rs > csharp_partial_type_reference_keeps_each_fragment_as_a_low_confidence_candidate | ✅ COMPLIANT |
| Namespace QNames | Namespaced method qualified name path-prefixed | csharp.rs > block_namespace_with_nested_class | ✅ COMPLIANT |
| Unresolved Preserved | BCL type reference unresolved with reason | csharp_graph_fixture.rs (System.Object case) | ⚠️ PARTIAL |
| Unresolved Preserved | Every listed unresolved case carries a reason | (none found combining all cases) | ❌ UNTESTED |
| Relationship Kind Per-Language | TS class extends/implements unchanged | relationship_graph_fixture.rs | ✅ COMPLIANT |
| Relationship Kind Per-Language | C# attribute produces decorates | csharp.rs > attributes_produce_decorates_with_own_span | ✅ COMPLIANT |
| Resolution Language Filter | Cross-file call within one language | resolve.rs > typescript_cross_file_calls_still_resolve_in_any_parse_order | ✅ COMPLIANT |
| Resolution Language Filter | Same-name symbols never cross-resolve | resolve.rs > language_filter_prevents_cross_language_resolution; mixed_language.rs | ✅ COMPLIANT |
| kind_matches Trait | TypeScript kind_matches unchanged | languages/mod.rs > typescript_analyzer_kind_matches_matches_pre_refactor_table | ✅ COMPLIANT |
| kind_matches Trait | C# and TS disagree without cross-contamination | (none found) | ❌ UNTESTED |
| Base List Disambiguation | Base-list class resolves as Inherits | resolve.rs > csharp_base_list_resolves_classes_and_interfaces_across_files | ✅ COMPLIANT |
| Base List Disambiguation | Base-list interface resolves as Implements | resolve.rs > csharp_base_list_resolves_classes_and_interfaces_across_files | ✅ COMPLIANT |
| Base List Disambiguation | Unresolved base-list entry never guessed | resolve.rs > csharp_unresolved_base_list_is_preserved_without_a_guess | ✅ COMPLIANT |
| internal Visibility | internal resolves at same confidence as public | resolve.rs > csharp_visibility_does_not_change_unambiguous_call_confidence | ✅ COMPLIANT |
| internal Visibility | internal is not enforced as a boundary | resolve.rs > csharp_visibility_does_not_change_unambiguous_call_confidence | ✅ COMPLIANT |
| Stable Symbol Key | Non-partial key byte-identical to pre-Phase-5 | repo.rs > symbol_key_none_matches_pre_migration_golden_hash | ✅ COMPLIANT |
| Stable Symbol Key | Two partial fragments distinct keys | csharp.rs > partial_fragments_in_one_file_get_distinct_ordinals | ✅ COMPLIANT |
| Stable Symbol Key | Partial fragments across files keep identity | partial_identity.rs; repo.rs > symbol_key_partial_ordinal_disambiguates | ✅ COMPLIANT |
| Analyzer Seam | TypeScript extraction unaffected | relationship_extraction.rs + relationship_graph_fixture.rs | ✅ COMPLIANT |
| Analyzer Seam | C# file analyzed by own analyzer | csharp_graph_fixture.rs | ✅ COMPLIANT |
| Visibility Enum | TypeScript symbols default visibility | (none found) | ❌ UNTESTED |
| Visibility Enum | C# visibility independent of export state | csharp.rs > visibility_matrix_all_six_levels | ⚠️ PARTIAL |
| partial/record Modifiers | record class is Class+is_record | csharp.rs > records_map_to_class_or_struct_with_is_record | ✅ COMPLIANT |
| partial/record Modifiers | record struct is Struct+is_record | csharp.rs > records_map_to_class_or_struct_with_is_record | ✅ COMPLIANT |
| partial/record Modifiers | partial class flagged without merging | csharp.rs > partial_fragments_in_one_file_get_distinct_ordinals | ✅ COMPLIANT |
| Migration 0004 | Applies without wiping populated index | migrations.rs > migration_0004_applies_without_wiping_populated_v3_data | ✅ COMPLIANT |
| Migration 0004 | Doctor reports post-migration schema version | vertical_slice.rs > doctor_reports_fts5 | ✅ COMPLIANT |
| .cs Discovery | New project indexes C# by default | vertical_slice_csharp.rs | ✅ COMPLIANT |
| .cs Discovery | Existing explicit language list unaffected | (none found) | ❌ UNTESTED |

**Compliance summary**: 35/46 scenarios compliant (3 partial, 8 untested, 0 failing)

### Correctness (Static Evidence)
| Requirement | Status | Notes |
|------------|--------|-------|
| C# Symbol Extraction | ✅ Implemented | Covered by csharp.rs unit tests |
| using Directives | ⚠️ Gaps | Directive recording tested; static/alias call-side behaviors under-tested |
| Base List Pending | ✅ Implemented | |
| Calls/Constructs | ✅ Implemented | |
| Attributes Decorates | ✅ Implemented | |
| Visibility Round-Trip | ❌ Incomplete | Stored/extracted, but `codekurve symbol` does not print visibility |
| Generics Structural | ⚠️ Partial | Fingerprint covered; instantiation-site case untested |
| Partial Not Merged | ✅ Implemented | |
| Namespace QNames | ✅ Implemented | |
| Unresolved Preserved | ⚠️ Partial | Core path covered; combined fixture missing |
| Relationship Kind Per-Language | ✅ Implemented | |
| Resolution Language Filter | ✅ Implemented | |
| kind_matches Trait | ⚠️ Partial | TS table tested; C# direct coverage missing |
| Base List Disambiguation | ✅ Implemented | |
| internal Visibility | ✅ Implemented | |
| Stable Symbol Key | ✅ Implemented | |
| Analyzer Seam | ✅ Implemented | |
| Visibility Enum | ⚠️ Partial | C# matrix exists; TS Default assertion missing |
| partial/record Modifiers | ✅ Implemented | |
| Migration 0004 | ✅ Implemented | |
| .cs Discovery | ⚠️ Partial | Default path covered; explicit-list regression missing |

### Coherence (Design)
| Decision | Followed? | Notes |
|----------|-----------|-------|
| same_resolution_domain filter | ✅ Yes | |
| LanguageAnalyzer 3 methods | ✅ Yes | |
| Base-list unresolved until resolve | ✅ Yes | |
| C# Imports own resolve branch | ✅ Yes | |
| Dotted namespace symbol names | ✅ Yes | |
| CLI prints visibility when non-default | ❌ No | `commands.rs`/`query.rs` omit visibility/modifiers |
| No Defines edges; Contains only | ✅ Yes | |
| partial_ordinal not persisted as column | ✅ Yes | |
| internal never lowers confidence | ✅ Yes | |
| record struct via struct child | ✅ Yes | |

### Issues Found
**CRITICAL**:
1. Visibility round-trip to query output is missing: design and spec require `codekurve symbol` to report distinct visibility values, but CLI output never prints `visibility:`/`modifiers:`.
2. `using static` call-side behavior lacks a covering test that `Calculate()` remains unresolved rather than silently resolving by bare name.
3. Alias-qualified references lack a covering test that they land in `unresolved_references` with an explicit reason (directive recording alone is insufficient).

**WARNING**:
4. CSharpAnalyzer::kind_matches has no direct test; base-list path bypasses it.
5. Generic instantiation-site type-argument non-resolution is untested.
6. Existing projects with explicit language lists excluding csharp lack a regression test.
7. TypeScript Visibility::Default and C# is_exported=false pairing lack explicit assertions.

**SUGGESTION**:
8. Add a combined unresolved-cases fixture (target-typed new, alias-qualified, unresolved base).
9. Strengthen cross-file partial member retention coverage.

### Verdict
FAIL
Independent verification found required scenario gaps and a design/spec mismatch on visibility query output despite green workspace tests and completed tasks.
