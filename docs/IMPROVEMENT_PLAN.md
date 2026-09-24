# Plan de mejoras — codeKurve (auditoría de código)

## Contexto

Revisión del workspace Rust (~21k líneas, 7 crates: core, analysis, store, codekurve, mcp, tui, bin).
Objetivo: detectar vulnerabilidades, problemas de eficiencia y de arquitectura, y ordenar el trabajo por impacto.

Valoración honesta primero: el proyecto está en buen estado. `unsafe_code = "forbid"`, SQL siempre
parametrizado (los dos `format!` en `crates/codekurve-store/src/repo.rs:602,924` solo generan
placeholders `?`), el export HTML escapa todo (`export.rs:480 esc()`), BFS acotado por
profundidad/nodos/aristas/tiempo, `cargo-deny` en CI, y el indexado ya cumple sus presupuestos
(10k archivos en ~3.4 s, `docs/PERFORMANCE.md`). Los hallazgos de abajo son reales, pero no hace
falta un rediseño. **No recomiendo** reescrituras grandes (dividir crates, cambiar SQLite, etc.).

---

## P0 — Bug de seguridad/corrección: `ignore.patterns` nunca se aplica — ✅ HECHO

**Hallazgo (confirmado):** `Config::ignore.patterns` (`crates/codekurve-core/src/config.rs:162-185`)
declara exclusiones por defecto (`**/node_modules/**`, `**/dist/**`, `**/build/**`, `**/*.min.js`,
`**/.env`, `**/secrets.*`, `*.pem`, `*.key`…), pero **ningún código lo lee**.
`discovery_options()` (`crates/codekurve/src/commands.rs:1154`) no lo pasa y
`DiscoveryOptions` (`crates/codekurve-analysis/src/discovery.rs:22`) no tiene campo para ello.
Solo se excluye lo que diga `.gitignore`.

Impacto:
- `docs/SECURITY_MODEL.md` promete que los patrones sensibles se excluyen: es falso.
  Ej.: `src/secrets.ts` o `config/secrets.cs` se indexa (nombres y spans en `index.db`, y el
  snippet se sirve por MCP a un agente).
- Eficiencia: un proyecto sin `.gitignore` (o con `dist/`/`build/` no ignorados) indexa
  `node_modules` y bundles minificados → puede disparar `max_total_files` o inflar el índice.
- El mensaje de error `error.rs:33` sugiere al usuario editar `ignore.patterns`, que no hace nada.

Cambio:
1. Añadir `exclude_patterns: Vec<String>` a `DiscoveryOptions`.
2. En `discover()`, usar `ignore::overrides::OverrideBuilder` (ya dependencia vía crate `ignore`)
   con cada patrón negado (`!pattern`), `builder.overrides(...)`. Patrón inválido → error claro.
3. Rellenarlo en `discovery_options()` desde `config.ignore.patterns`.
4. Actualizar las 2 construcciones de test (`discovery.rs:118`, `incremental.rs:439`).
5. Tests: fixture sin git con `node_modules/x.ts`, `dist/a.js`, `secrets.ts` → no descubiertos;
   patrón custom en config respetado.
6. Considerar: el watcher/incremental usan las mismas `options`, así que heredan el arreglo;
   un archivo ya indexado que pasa a estar excluido debe borrarse del índice (verificar que
   `incremental::detect` lo trata como "deleted"; añadir test).
7. Subir `ANALYZER_VERSION`/config hash si hace falta para forzar reindex en índices existentes
   (revisar `repo::config_hash` — si el hash del config ya cubre esto, no hace falta nada).

## P1 — Seguridad de la cadena de distribución — ✅ HECHO (salvo lo indicado)

**1a. El instalador no verifica checksums.** `release.yml` publica `SHA256SUMS`, pero
`install.sh:72` e `install.ps1:58` descargan el binario y lo ejecutan sin verificarlo.
Además `codekurve update` (`crates/codekurve/src/update.rs`) hace `curl …/main/install.sh | sh`,
es decir, ejecuta el script de la rama `main` (mutable) y no uno fijado a la versión.
- Descargar `SHA256SUMS` del mismo release y verificar (`sha256sum -c`/`shasum -a 256`;
  `Get-FileHash` en PowerShell). Fallo → abortar sin mover el binario.
- Valorar (opcional, más trabajo): attestations de GitHub (`actions/attest-build-provenance`)
  para que el usuario pueda verificar con `gh attestation verify`.

  **Estado:** verificación de checksum hecha en ambos scripts (probada contra el release v0.2.11
  real: caso correcto, checksum que no coincide, entrada ausente, sin herramienta de hash y los
  fallbacks `shasum`/`openssl`; `install.ps1` ejecutado con pwsh 7.4). Attestations añadidas al job
  `publish` (sin probar hasta el próximo tag). **No se cambia** que `update` use el script de
  `main`: fijarlo a la versión instalada impediría que los arreglos del instalador lleguen a los
  usuarios, y un tag no es más inmutable que `main` para quien controle el repo. La protección
  real contra un release comprometido son las attestations, no esto.

**1b. Actions sin fijar por SHA.** *(Hecho: fijadas a la última versión de su misma mayor, v4; el
comentario del pin de `cargo-deny-action` decía `v2.1.1` pero el SHA es el de la etiqueta
flotante `v2`: corregido el comentario, no el SHA.)* Solo `cargo-deny-action` está fijado. `actions/checkout@v4`,
`dtolnay/rust-toolchain@stable`, `upload/download-artifact@v4` en `ci.yml` y `release.yml`
(que tiene permiso `contents: write`) → fijar a SHA con comentario de versión.
En `ci.yml` declarar `permissions: contents: read` explícito.

**1c. Documentación desalineada con la realidad.** `docs/SECURITY_MODEL.md` dice "no public
publish step", pero `release.yml:145` hace `gh release create`. Corregir junto con 1a.

## P1 — Robustez del servidor MCP — ✅ HECHO

Archivo: `crates/codekurve-mcp/src/tools.rs`, `server.rs`, `lib.rs`.
- **Trabajo bloqueante en runtime `current_thread`.** Todas las tools son `fn` síncronas que
  hacen SQLite/BFS y, en `codekurve_reindex`, un reindex completo (segundos). Mientras tanto el
  loop de stdio no procesa nada (ni `ping` ni cancelaciones). Mover el cuerpo de cada tool a
  `tokio::task::spawn_blocking` (la `Session` ya está tras `Mutex`; envolver en `Arc`).
  Mínimo imprescindible: `codekurve_reindex`, `trace_path`, `analyze_impact`.
- **Mutex envenenado = servidor muerto.** `self.session.lock().unwrap()` en cada tool: si una
  tool entra en pánico, todas las llamadas siguientes también. Sustituir por un helper
  `lock_session()` que devuelva `McpError::internal_error` (o `into_inner()` tras el pánico).
- ~~`ProtocolVersion::V_2024_11_05` fijado~~ — **descartado tras comprobarlo:** rmcp 2.2
  (`negotiate_protocol_version`) devuelve la versión que pide el cliente si la conoce (hasta
  `2025-11-25`); el valor del servidor solo es el fallback para versiones desconocidas.

  **Estado:** helper `CodeKurve::blocking` (`server.rs`) usado por las 14 tools. Además del
  bloqueo, resultó que un pánico dejaba al cliente **sin respuesta** (rmcp hace `tokio::spawn`
  de cada petición y la tarea moría en silencio), no solo el mutex envenenado. Recuperación:
  reabrir la `Session` desde disco. Tests: `server::tests` (ambos fallan con la versión
  anterior). Las llamadas siguen serializadas sobre la única conexión SQLite; es intencionado.

## P2 — Eficiencia de consultas — ✅ HECHO (lo de `load_adjacency`; ver abajo)

- **`load_adjacency` carga el grafo entero en cada `trace`/`impact`**
  (`crates/codekurve-store/src/traverse.rs:35`): lee todas las relaciones del proyecto y aloca
  `String`s por arista, aunque el BFS luego se corte a 500 nodos. En repos grandes esto domina
  la latencia (y no está medido: `PERFORMANCE.md` reconoce que no hay benchmark de queries).
  Propuesta: expansión perezosa — consulta preparada `SELECT … WHERE project_id=? AND
  source_symbol_id=?` (o `target_symbol_id` para reverse) por nodo visitado; verificar que
  existen índices en `relationships(project_id, source_symbol_id)` y `(project_id,
  target_symbol_id)` en `migrations.rs` (crear migración si falta).
  **Primero medir** (añadir tier de latencia a `scripts/bench.py`) y solo cambiar si se nota.

  **Resultado de medir (`scripts/bench_queries.py`):** `load_adjacency` no era lo peor. En el tier
  large *todas* las tools tardaban 44-107 ms y crecían con el proyecto: (1) la base nunca tenía
  estadísticas (`ANALYZE`), así que SQLite elegía índices malos (`search` recorría 53k símbolos);
  (2) cada tool calculaba 4 `COUNT(*)` para leer `pending_files`; (3) `load_adjacency`. Arreglados
  los tres: todas < ~1 ms p95 e independientes del tamaño. Detalle en `docs/PERFORMANCE.md`.
- **Parsing secuencial.** No hay paralelismo (`rayon`/threads) en `incremental.rs`/`commands.rs`.
  Dado que ya se cumplen los presupuestos, es **mejora opcional**, no prioridad: paralelizar
  solo `extract::analyze` con `rayon` y mantener un único escritor SQLite (ADR 0008).
- ✅ *(Hecho: también afectaba a MCP `get_symbol`, que devolvía líneas desplazadas con
  `stale: false` pese a que la spec lo prohíbe.)* **Snippet posiblemente obsoleto marcado como "(live)"** (`commands.rs:1171 snippet()`):
  solo se comprueba que el span quepa en el archivo; si cambió pero sigue siendo largo, se
  devuelve texto incorrecto como "live". Comparar `content_hash` del archivo con el almacenado
  (ya existe `repo::content_hash`) antes de servirlo.

## P2 — Arquitectura y mantenibilidad

- ◐ *(Parcial: `incremental::DetectError` elimina el `contains("max_total_files")` del watcher,
  con test de la clasificación. El resto de firmas `String` se deja: migrarlas en bloque es un
  refactor grande sin fallo concreto detrás; mejor tiparlas cuando se toque cada módulo.)*
  **Modelo de errores inconsistente:** core/store usan `thiserror`; el crate `codekurve` usa
  `Result<_, String>` (~50 firmas en `commands.rs`, `install.rs`, `incremental.rs`, `watch.rs`,
  `update.rs`) y se detectan errores por texto: `watch.rs:124` `e.contains("max_total_files")`.
  Introducir un `enum AppError` con `thiserror` en `crates/codekurve/src/` y migrar
  incrementalmente, empezando por `incremental`/`watch` (quitar el `contains`).
- ✅ *(Hecho: sonda común `query::engine_checks`.)* **Lógica duplicada:** `query::doctor` y `commands::doctor` tienen implementaciones separadas
  (`query.rs:649` lo documenta). Extraer las comprobaciones comunes.
- **`repo.rs` (2.9k líneas, ~1.5k de código):** dividir en submódulos `store/repo/{write,
  relationships, search, lookup}.rs` sin cambiar la API pública. Solo mecánico; hacerlo cuando
  se toque ese archivo, no como tarea aislada.
- **`commands.rs` mezcla lógica y salida por consola** (`println!`). Mover formateo a la capa
  bin/CLI a medida que se refactorice; no urgente.
- ✅ *(Hecho.)* **Docs obsoletas:** `docs/ARCHITECTURE.md` sigue diciendo "Phase 0 reality: crates vacíos",
  "clap no introducido", y el grafo de dependencias no coincide (mcp y tui dependen del crate
  `codekurve`, no de core/store). Reescribir esa sección.
- **Observabilidad:** no hay `tracing`; el watcher usa `println!/eprintln!`. Añadir `tracing` +
  `tracing-subscriber` a stderr (útil sobre todo para `watch` y `mcp`). Prioridad baja.

## P3 — CI / calidad

- Cache de dependencias (`Swatinem/rust-cache`, fijado por SHA): la matriz de 3 SO compila
  todo desde cero en cada push. El comentario de CI lo difiere "hasta medir lentitud"; medir.
- Añadir `cargo test --doc` o dejar documentado por qué no.
- Fijar MSRV (`rust-version`) ahora que hay releases públicas.

---

## Orden de ejecución propuesto

1. ~~P0 `ignore.patterns`~~ — hecho (`discovery.rs` + `tests/ignore_patterns.rs`).
2. ~~P1 checksums del instalador + fijar Actions + corregir SECURITY_MODEL.md~~ — hecho.
3. ~~P1 MCP `spawn_blocking` + helper de lock~~ — hecho.
4. ~~Benchmark de latencia de queries → decidir P2 adjacency perezosa~~ — hecho.
5. P2 snippet con hash, `AppError`, docs de arquitectura.
6. Resto (split de `repo.rs`, tracing, rayon, CI cache) oportunistamente.

## Verificación

- Por cada PR: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
- P0: tests nuevos en `discovery.rs` + test de integración en `crates/codekurve-bin/tests/`
  (indexar fixture con `node_modules/` y `secrets.ts` sin `.gitignore`, comprobar con
  `codekurve search` que no aparecen).
- P1 instalador: `sh -n install.sh`; probar manualmente con un binario corrupto → debe abortar.
- P1 MCP: test en `crates/codekurve-mcp/tests/` que lance `reindex` y en paralelo `project_status`
  y compruebe que el segundo responde; test de que tras un pánico simulado el servidor sigue.
- P2 queries: `python3 scripts/bench.py --tier large` antes/después.

## Hallazgo posterior — ✅ HECHO: archivo muy anidado tumbaba el indexador

Los extractores del AST son recursivos sin límite. Reproducido: un `.ts` de ~20 KB (10k `[`)
abortaba `codekurve index`, y 5k niveles mataban el servidor MCP en `codekurve_reindex`. Arreglo:
análisis en un hilo con pila de 256 MiB reservada por lote, más un límite de 10k niveles (el
archivo se indexa vacío con un aviso). `SECURITY_MODEL.md` corregido: prometía timeouts,
presupuestos de memoria y logs con redacción que no existían.
