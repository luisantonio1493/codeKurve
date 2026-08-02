# Pilot Report — Phase 8

**Date**: 2026-08-02
**Decision**: **Continue.**

Phase 8 of `CODEKURVE_MASTER_PLAN.md` asks for a pilot on real repositories,
a set of measurements, and a decision. This is that record. Unlike phases 5–7
it did not run through the SDD flow — it is an evaluation phase, not an
implementation one, so there is no `openspec/changes/` folder for it.

## Selection

The plan asked for one Angular repo, one .NET repo, and one hard real
workflow. Three repositories were used, two of them real production or
near-production code rather than fixtures:

| repo | stack | files | why |
|---|---|---|---|
| `futbolsinfronterasAngular` | Angular (standalone components) | 22 | the Angular side |
| `MinimalApi-main` | .NET 9, minimal APIs, EF Core | 22 | modern .NET style |
| `private-production-dotnet-solution` | .NET, classic ASP.NET Core, 6-project solution | 350 | **production**, layered (API/BL/DAL/Services), plus an Azure Function |

The third was added mid-pilot and proved decisive: it is a different
*generation* of ASP.NET than the sample project, and it surfaced gaps the
sample structurally could not.

## Measurements

| metric | Angular (22 files) | .NET sample (22) | .NET production (350) |
|---|---|---|---|
| full index | 47 ms | 66 ms | **0.99 s** |
| incremental, no changes | 12 ms | 12 ms | **0.01 s** |
| peak memory, index | 4.6 MB | 7.2 MB | **57 MB** |
| peak memory, query | 2.6 MB | 2.8 MB | — |
| query latency (search/callers/impact) | ~7–8 ms | ~7–8 ms | — |
| symbols | 65 | 150 | **5 517** |
| relationships | 159 | 339 | **18 993** |

Framework recognition on the production app, which is the clearest evidence
that phase 7 works at scale rather than only on fixtures:

```
handlesroute  246      persiststo    203      registeredas   30
inherits       36      implements     34      triggers        2
```

### Precision

For the workflow *"I am about to change `TodoItem` — what breaks?"*:

| | codekurve | `grep -rl TodoItem` |
|---|---|---|
| tool calls | 5 | 1 |
| files identified | 7–8 | 16 |
| false positives | **0** | ~8 (EF migration/designer files, `TodoItemAudit`, `User`) |
| source files read in full | **0** | all 16, to filter them |

An agent (OpenCode, GPT-5.6) answering *"what does `TransformAsync` do?"* used
12 structured calls and read no file in full. It correctly detected that two
different `TransformAsync` methods existed and avoided explaining the wrong
one.

The closed-list design for framework recognition was validated adversarially:
the production app calls `AddRange` 50 times, `AddHour` 19, `AddDay` 15. Had
the catalogue matched by `Add*` prefix instead of exact names, it would have
manufactured roughly 100 false edges in that repository alone.

## Bugs found

The pilot paid for itself. All were found by running on real code, none by
fixtures, and all are fixed:

1. **`apply_incremental` foreign-key violation** — editing any file that
   another file imports crashed incremental indexing. This broke the single
   most common workflow (`codekurve index`/`watch` during development) and no
   fixture had caught it, because fixtures never edit a file with inbound
   references.
2. **`trace`/`impact` printed raw `sym-*` ids** — the two commands that answer
   blast-radius questions returned identifiers with no way to resolve them,
   while `callers`/`references` had always resolved names.
3. **Unresolved references were invisible** — the analyzer recorded *why* it
   declined to resolve a reference, but only a count was ever exposed. An
   agent asking what implements an external interface got silence and fell
   back to reading source. Now queryable via `codekurve unresolved` and the
   `find_unresolved` MCP tool.

Two further gaps in .NET recognition were measured and closed:
`RegisteredAs` was **0** on a real ASP.NET project (the catalogue knew only
`AddScoped`/`AddTransient`/`AddSingleton`), and the classic pre-minimal-API
host builder was entirely unrecognized. After both fixes the production app
reports 30 registrations, including `UseStartup<Startup>` resolving to its
real startup class and `AddDbContext<T>` linking to both real `DbContext`s.

## Honest limitations

- **Angular is far less validated than .NET.** The .NET side now has a
  350-file production application; Angular was only exercised on a 22-file
  app. The strategy of being strongest on C# *and* Angular is currently
  half-proven.
- **Developer satisfaction was not formally measured.** The evidence is
  qualitative: the author's own use, plus two agents (Codex, OpenCode)
  reporting that the tool helped and correctly flagging its limits.
- **Bare feature registrations do not become edges.** `AddOpenApi()`-style
  calls are recognized but their targets are framework features with no
  project symbol, so they remain unresolved references. Documented in
  `docs/FRAMEWORKS.md`.
- **Multi-project solutions are flattened.** `private-production-dotnet-solution` has six
  `.csproj` projects; codekurve indexes every `.cs` file below the root as one
  flat project. Project references and assembly-level `internal` scope are not
  modelled (`docs/LANGUAGES.md`).

## Decision: continue

The tool does what it claims on real repositories in both target ecosystems,
performance shows no ceiling at 350 files / 19k relationships, and the pilot
found three real defects that fixtures had not. Nothing measured argues for
stopping or for a redesign.

Adjustments carried forward rather than blocking:

- Validate Angular against a production-sized application, to bring that side
  to parity with the .NET evidence.
- Continue widening the framework catalogues from real code, not from memory
  — every gap closed in this pilot was found by surveying an actual
  repository.
