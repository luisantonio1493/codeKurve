# CodeKurve — Plan maestro de arquitectura, producto e implementación

> Documento de arranque para Codex, Claude Code u otro agente de programación.
>
> **Estado:** propuesta inicial  
> **Nombre del repositorio:** `codekurve`  
> **Lenguaje principal:** Rust  
> **Tipo de producto:** indexador local de código y grafo de dependencias para humanos y agentes de programación  
> **Prioridad inicial:** rendimiento, privacidad, auditabilidad, simplicidad operativa y utilidad real en proyectos empresariales  
> **Fecha base del plan:** 2026-07-21

---

# 0. Instrucciones para el agente que implemente CodeKurve

Este documento es la fuente inicial de verdad del proyecto. Antes de escribir código:

1. Lee el documento completo.
2. No intentes implementar todo el roadmap en un solo cambio.
3. Comienza por la **Fase 0** y la **Fase 1**.
4. Mantén el proyecto compilando y con pruebas en cada commit.
5. No agregues dependencias sin justificar:
   - propósito;
   - licencia;
   - mantenimiento;
   - costo en tamaño del binario;
   - impacto de seguridad;
   - alternativa considerada.
6. No agregues conectividad de red, telemetría ni llamadas a servicios externos.
7. No uses un LLM para construir el grafo.
8. No almacenes secretos ni copies innecesariamente el código fuente dentro de SQLite.
9. No implementes un framework genérico antes de completar un vertical slice funcional.
10. Toda decisión importante debe registrarse como ADR en `docs/adr/`.
11. Cuando exista ambigüedad, elige la solución más pequeña que:
    - preserve la arquitectura;
    - permita pruebas;
    - no cierre el camino a extensiones futuras.
12. El código, los nombres públicos, los mensajes técnicos y la documentación del repositorio deben escribirse en inglés. Este plan está en español para facilitar la discusión inicial.
13. Después de cada fase:
    - ejecuta `cargo fmt --check`;
    - ejecuta `cargo clippy --workspace --all-targets --all-features -- -D warnings`;
    - ejecuta `cargo test --workspace`;
    - registra resultados;
    - actualiza `CHANGELOG.md` bajo `Unreleased`;
    - actualiza el roadmap.
14. No simules resultados de benchmarks. Los números deben provenir de ejecuciones reproducibles.
15. No presentes una relación como definitiva si fue resuelta mediante heurística. Toda relación debe incluir procedencia y confianza.

---

# 1. Resumen ejecutivo

CodeKurve será una herramienta local que analiza un repositorio, extrae símbolos y relaciones del código, persiste un índice en SQLite y expone consultas rápidas mediante CLI y MCP.

Su propósito no es sustituir al compilador, al Language Server, a `ripgrep` ni al control de versiones. Su propósito es evitar que un humano o un agente tenga que reconstruir repetidamente la estructura del proyecto archivo por archivo.

CodeKurve debe responder preguntas como:

- ¿Dónde está definido este símbolo?
- ¿Dónde se usa?
- ¿Qué métodos llama?
- ¿Quién llama este método?
- ¿Qué clases implementan esta interfaz?
- ¿Qué módulos dependen de este archivo?
- ¿Qué ruta conecta un endpoint con una capa de datos?
- ¿Cuál es el posible radio de impacto de cambiar este símbolo?
- ¿Cuáles son los principales entry points y subsistemas del repositorio?
- ¿Qué partes del índice están desactualizadas?

La primera versión será deliberadamente pequeña:

- un solo repositorio;
- ejecución completamente local;
- Rust;
- SQLite;
- Tree-sitter;
- TypeScript/JavaScript como primer lenguaje;
- C# como segundo lenguaje;
- CLI;
- MCP por `stdio`;
- indexación completa;
- actualización incremental;
- consultas de símbolos y relaciones;
- sin UI web;
- sin cloud;
- sin embeddings;
- sin vector database;
- sin LLM;
- sin análisis de documentos, PDF, imágenes o video.

El objetivo no es crear una copia completa de Graphify o CodeGraph. El objetivo es crear una herramienta interna que pueda ser auditada, aprobada, ejecutada y mantenida dentro de una empresa.

---

# 2. Identidad del producto

## 2.1 Nombre

**CodeKurve**

Nombre técnico del binario:

```text
codekurve
```

Nombre del repositorio:

```text
codekurve
```

Nombre del directorio de datos local:

```text
.codekurve/
```

Prefijo de variables de entorno:

```text
CODEKURVE_
```

Prefijo de herramientas MCP:

```text
codekurve_
```

## 2.2 Interpretación del nombre

“Kurve” evoca una curva o representación gráfica en lenguas escandinavas. El nombre comunica:

- código;
- conexiones;
- caminos;
- curvas de dependencia;
- grafo;
- navegación estructural.

Antes de publicar el proyecto fuera de la empresa se debe realizar:

- búsqueda de marcas;
- búsqueda de dominios;
- búsqueda de paquetes existentes;
- revisión de nombres similares;
- decisión final de capitalización.

Hasta entonces, usar **CodeKurve** como nombre de producto y `codekurve` como identificador técnico.

## 2.3 Posicionamiento

Descripción de una línea:

> CodeKurve is a local-first, high-performance code graph for developers and coding agents.

Descripción interna empresarial:

> CodeKurve is a local static-analysis indexer for code navigation, dependency tracing, and change-impact assessment.

La segunda descripción debe usarse en procesos de seguridad y aprobación. No es necesario presentarlo inicialmente como “una herramienta de IA”, porque su funcionamiento principal es análisis estático determinista.

---

# 3. Problema

Los agentes de programación y los desarrolladores suelen reconstruir el contexto de un repositorio mediante:

- listado de directorios;
- búsqueda textual;
- lectura de archivos;
- seguimiento manual de imports;
- inspección de interfaces;
- reconstrucción de rutas de llamadas;
- búsqueda repetida de referencias;
- lectura de configuración de dependency injection;
- análisis manual de entry points.

Esto genera:

- muchas operaciones de I/O;
- muchas llamadas a herramientas;
- consumo innecesario de tokens;
- respuestas incompletas;
- omisión de dependencias indirectas;
- tiempo perdido entre sesiones;
- resultados inconsistentes dependiendo de la estrategia del agente.

CodeKurve preconstruirá y mantendrá un grafo consultable.

---

# 4. Visión

## 4.1 Visión a corto plazo

Un desarrollador debe poder ejecutar:

```bash
codekurve init
codekurve index
codekurve search "EligibilityService"
codekurve callers "EligibilityService.getEligibility"
codekurve mcp
```

y obtener resultados útiles sin instalar servicios, bases externas o runtimes adicionales.

## 4.2 Visión a mediano plazo

CodeKurve debe convertirse en una capa común de inteligencia estructural para:

- Claude Code;
- Codex;
- OpenCode;
- VS Code;
- herramientas internas;
- scripts de análisis;
- pipelines de CI;
- análisis de pull requests.

## 4.3 Visión a largo plazo

Sin comprometer el MVP, la arquitectura debe permitir:

- soporte para más lenguajes;
- analizadores semánticos opcionales;
- análisis de frameworks;
- comparación entre commits;
- impacto de pull requests;
- reglas arquitectónicas;
- grafo multi-repositorio;
- exportación;
- visualización;
- servidor local opcional;
- integración con herramientas corporativas.

---

# 5. Principios no negociables

## 5.1 Local-first

Todo debe ejecutarse localmente. La versión inicial no debe:

- realizar requests HTTP;
- enviar telemetría;
- consultar APIs;
- subir código;
- requerir cuentas;
- utilizar SaaS;
- depender de modelos externos.

## 5.2 Determinismo

Dado:

- el mismo código;
- la misma configuración;
- la misma versión de CodeKurve;
- las mismas gramáticas;

el índice debe ser reproducible.

## 5.3 Procedencia explícita

Toda relación debe indicar cómo se obtuvo:

```text
extracted
resolved
heuristic
external
```

En el MVP solo deben utilizarse:

```text
extracted
resolved
heuristic
```

## 5.4 Confianza explícita

Toda relación tendrá un nivel:

```text
exact
high
medium
low
unresolved
```

Las herramientas no deben ocultar la incertidumbre.

## 5.5 Índice descartable

La base de datos debe poder eliminarse y reconstruirse. Nunca debe ser la única fuente de verdad.

## 5.6 Escrituras transaccionales

Una indexación fallida no debe dejar un archivo parcialmente actualizado.

## 5.7 Resultados acotados

Las herramientas MCP y CLI deben incluir:

- límites;
- paginación;
- profundidad máxima;
- truncamiento explícito;
- conteos totales;
- advertencias.

## 5.8 Seguridad por defecto

- Sin red.
- Sin telemetría.
- Respeto de `.gitignore`.
- Exclusiones adicionales.
- No seguir symlinks por defecto.
- No indexar binarios.
- Límite de tamaño por archivo.
- Rutas normalizadas.
- No permitir salir del project root.
- Logs sin contenido sensible por defecto.

## 5.9 Rendimiento medible

No usar “rápido” como afirmación de marketing sin benchmarks.

## 5.10 Simplicidad antes de abstracción

No construir un motor genérico de plugins hasta que existan al menos dos analizadores funcionales.

---

# 6. Objetivos de la versión 0.1

La versión `0.1.0` debe:

1. Inicializar un proyecto.
2. Descubrir archivos respetando ignores.
3. Detectar lenguaje por extensión.
4. Indexar TypeScript y JavaScript.
5. Extraer:
   - archivos;
   - imports;
   - exports;
   - clases;
   - interfaces;
   - funciones;
   - métodos;
   - propiedades relevantes;
   - invocaciones sintácticas;
   - herencia;
   - implementación de interfaces cuando sea resoluble.
6. Persistir el índice en SQLite.
7. Crear FTS para símbolos.
8. Buscar símbolos.
9. Recuperar definición y fragmento de código actual.
10. Consultar referencias.
11. Consultar callers y callees.
12. Trazar un camino limitado entre símbolos.
13. Ejecutar análisis básico de impacto inverso.
14. Mantener el índice con watcher.
15. Exponer MCP por `stdio`.
16. Proveer diagnóstico local.
17. Operar sin red.
18. Tener pruebas unitarias, de integración y benchmarks básicos.
19. Distribuir un binario para macOS, Linux y Windows.
20. Documentar amenazas, almacenamiento y limitaciones.

---

# 7. No objetivos de la versión 0.1

No implementar todavía:

- comprensión natural de preguntas;
- embeddings;
- vector search;
- LLM inference;
- Neo4j;
- cloud;
- colaboración;
- sincronización entre máquinas;
- interfaz web;
- aplicación desktop;
- editor propio;
- refactoring automático;
- modificación de archivos;
- análisis completo de runtime;
- resolución perfecta de dispatch dinámico;
- soporte de 20 o 40 lenguajes;
- PDF;
- imágenes;
- video;
- audio;
- reuniones;
- diagramas;
- documentación empresarial externa;
- análisis de múltiples repositorios;
- reglas de compliance;
- análisis de secretos;
- análisis de vulnerabilidades;
- sustitución del compilador;
- sustitución del Language Server;
- generación automática de código.

---

# 8. Usuarios y casos de uso

## 8.1 Desarrollador

Necesita localizar símbolos y comprender dependencias.

## 8.2 Revisor de pull request

Necesita identificar el radio de impacto de un cambio.

## 8.3 Arquitecto

Necesita entender entry points, capas y dependencias cruzadas.

## 8.4 Agente de programación

Necesita recuperar contexto estructural en pocas llamadas.

## 8.5 Equipo de seguridad

Necesita confirmar que:

- no hay red;
- el índice es local;
- los archivos ignorados no se procesan;
- la herramienta es auditable;
- las dependencias están controladas.

## 8.6 Mantenedor de CodeKurve

Necesita:

- añadir lenguajes;
- ajustar reglas;
- reproducir bugs;
- medir precisión;
- publicar binarios;
- migrar esquemas.

---

# 9. Historias de usuario del MVP

## US-001 Inicialización

Como desarrollador, quiero inicializar CodeKurve en un repositorio para crear una configuración local segura.

Criterios:

```bash
codekurve init
```

debe crear:

```text
.codekurve/
├── config.toml
└── .gitignore
```

La base de datos puede crearse durante `index`.

## US-002 Indexación completa

Como desarrollador, quiero construir el índice completo.

```bash
codekurve index
```

Debe mostrar:

- archivos descubiertos;
- archivos ignorados;
- archivos soportados;
- errores de parsing;
- símbolos;
- relaciones;
- duración por etapa;
- ubicación de la base.

## US-003 Búsqueda

```bash
codekurve search EligibilityService
```

Debe devolver coincidencias ordenadas con:

- nombre;
- qualified name;
- tipo;
- archivo;
- líneas;
- score;
- lenguaje.

## US-004 Definición

```bash
codekurve symbol "EligibilityService.getEligibility"
```

Debe devolver:

- metadata;
- signature;
- source span;
- snippet actual;
- relaciones principales;
- staleness.

## US-005 Callers

```bash
codekurve callers "EligibilityService.getEligibility"
```

## US-006 Callees

```bash
codekurve callees "EligibilityService.getEligibility"
```

## US-007 Referencias

```bash
codekurve references "EligibilityService"
```

## US-008 Implementaciones

```bash
codekurve implementations "IMemberService"
```

## US-009 Camino

```bash
codekurve trace "EligibilityController" "MemberRepository"
```

## US-010 Impacto

```bash
codekurve impact "IMemberService" --depth 3
```

## US-011 Watcher

```bash
codekurve watch
```

Debe reindexar solamente archivos afectados.

## US-012 MCP

```bash
codekurve mcp
```

Debe iniciar un servidor MCP por `stdio`.

## US-013 Diagnóstico

```bash
codekurve doctor
```

Debe verificar:

- configuración;
- permisos;
- SQLite;
- FTS5;
- schema version;
- root;
- grammars;
- watcher;
- estado del índice;
- ausencia de red en configuración;
- archivos pendientes.

---

# 10. Arquitectura de alto nivel

```text
┌────────────────────────────────────────────────────────────┐
│                         Interfaces                         │
│                                                            │
│  CLI                         MCP stdio                      │
│   │                              │                         │
└───┼──────────────────────────────┼─────────────────────────┘
    │                              │
    └──────────────┬───────────────┘
                   ▼
┌────────────────────────────────────────────────────────────┐
│                    Application Services                    │
│                                                            │
│ project lifecycle | indexing | queries | diagnostics       │
└────────────────────────────────────────────────────────────┘
                   │
       ┌───────────┼──────────────┐
       ▼           ▼              ▼
┌────────────┐ ┌────────────┐ ┌──────────────┐
│ Discovery  │ │ Analysis   │ │ Query Engine │
│            │ │            │ │              │
│ ignores    │ │ parsers    │ │ search       │
│ hashes     │ │ resolvers  │ │ traversal    │
│ watcher    │ │ graph IR   │ │ impact       │
└────────────┘ └────────────┘ └──────────────┘
       │           │              │
       └───────────┴──────┬───────┘
                          ▼
                 ┌──────────────────┐
                 │ SQLite Store     │
                 │                  │
                 │ metadata         │
                 │ symbols          │
                 │ edges            │
                 │ FTS5             │
                 │ migrations       │
                 └──────────────────┘
```

---

# 11. Estrategia de workspace en Rust

No comenzar con quince crates. Iniciar con límites claros pero manejables.

## 11.1 Estructura inicial

```text
codekurve/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── README.md
├── CHANGELOG.md
├── SECURITY.md
├── CONTRIBUTING.md
├── LICENSES/
├── docs/
│   ├── ARCHITECTURE.md
│   ├── DATA_MODEL.md
│   ├── SECURITY_MODEL.md
│   ├── PERFORMANCE.md
│   ├── MCP.md
│   ├── ROADMAP.md
│   └── adr/
├── crates/
│   ├── codekurve/
│   │   └── src/
│   ├── codekurve-core/
│   │   └── src/
│   ├── codekurve-analysis/
│   │   └── src/
│   ├── codekurve-store/
│   │   └── src/
│   └── codekurve-mcp/
│       └── src/
├── fixtures/
│   ├── typescript/
│   ├── javascript/
│   └── mixed/
├── benches/
├── migrations/
└── .github/
    └── workflows/
```

## 11.2 Responsabilidades

### `codekurve`

Binary crate.

Responsable de:

- CLI;
- carga de configuración;
- composición de dependencias;
- códigos de salida;
- salida humana y JSON.

No debe contener lógica de parsing ni SQL complejo.

### `codekurve-core`

Tipos de dominio:

- `Project`;
- `FileRecord`;
- `Symbol`;
- `SymbolKind`;
- `Relationship`;
- `RelationshipKind`;
- `Confidence`;
- `Provenance`;
- `SourceSpan`;
- `LanguageId`;
- errores del dominio;
- traits principales.

No debe depender de CLI, SQLite ni MCP.

### `codekurve-analysis`

Responsable de:

- discovery;
- hash;
- detection;
- Tree-sitter;
- extracción;
- resolución;
- index planning;
- incremental invalidation;
- watcher.

Al comienzo puede contener módulos internos:

```text
discovery
hashing
languages
pipeline
resolution
watch
```

Separar en crates futuros solo cuando exista presión real.

### `codekurve-store`

Responsable de:

- conexión SQLite;
- migraciones;
- repositorios;
- transacciones;
- FTS;
- queries persistentes;
- integridad;
- schema diagnostics.

### `codekurve-mcp`

Responsable de:

- servidor MCP;
- schemas;
- adapters entre MCP y application services;
- límites de resultados;
- serialización segura.

## 11.3 Posible estructura futura

Solo después de `0.1`:

```text
codekurve-language-typescript
codekurve-language-csharp
codekurve-query
codekurve-bench
codekurve-protocol
```

---

# 12. Dependencias iniciales propuestas

Las versiones deben fijarse durante la implementación según el Rust estable aprobado. `Cargo.lock` debe incluirse.

## Runtime y CLI

```text
clap
serde
serde_json
toml
thiserror
anyhow
tracing
tracing-subscriber
```

## Async y MCP

```text
tokio
rmcp
```

Usar async únicamente donde aporte valor:

- MCP;
- coordinación;
- señales;
- watcher si es necesario.

No convertir todo el dominio o SQLite a async sin necesidad.

## Parsing

```text
tree-sitter
tree-sitter-typescript
tree-sitter-javascript
tree-sitter-c-sharp
```

Verificar compatibilidad exacta de versiones entre core y grammars.

## Discovery y cambios

```text
ignore
notify
blake3
```

## Paralelismo

```text
rayon
crossbeam-channel
```

No mezclar Rayon y Tokio indiscriminadamente. Regla inicial:

- Rayon para CPU-bound parsing.
- Tokio para MCP, lifecycle y señales.
- Un writer dedicado para SQLite.
- Channels acotados para backpressure.

## Persistencia

```text
rusqlite
```

Preferir `bundled` para tener una versión controlada de SQLite, sujeto a revisión de licencias y tamaño.

## Desarrollo

```text
tempfile
assert_cmd
predicates
insta
proptest
criterion
```

## Dependencias que no deben agregarse inicialmente

- Neo4j client.
- HTTP clients.
- OpenAI SDK.
- Anthropic SDK.
- vector database.
- ORM pesado.
- framework web.
- GUI.
- Tauri.
- Electron.
- parser generators adicionales sin caso concreto.
- plugin runtime dinámico.
- scripting runtime.

---

# 13. Toolchain

Crear:

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

Durante el primer commit se debe registrar en un ADR si se fija una versión específica.

Política:

- usar Rust estable;
- definir MSRV después de validar dependencias;
- no usar nightly en `0.1`;
- evitar `unsafe`;
- todo `unsafe` futuro requiere ADR y pruebas específicas.

---

# 14. Configuración

Archivo:

```text
.codekurve/config.toml
```

Propuesta:

```toml
version = 1

[project]
name = "my-project"
root = ".."

[index]
languages = ["typescript", "javascript"]
max_file_size_bytes = 2097152
follow_symlinks = false
include_hidden = false
store_source = false
hash_algorithm = "blake3"
worker_count = 0

[index.watch]
enabled = true
debounce_ms = 750
reconcile_on_start = true

[ignore]
respect_gitignore = true
respect_global_gitignore = true
patterns = [
  ".codekurve/**",
  "**/node_modules/**",
  "**/dist/**",
  "**/build/**",
  "**/.git/**",
  "**/*.min.js",
  "**/*.map",
  "**/.env",
  "**/.env.*",
  "**/secrets.*",
  "**/*.pfx",
  "**/*.pem",
  "**/*.key"
]

[storage]
database = ".codekurve/index.db"
journal_mode = "wal"
busy_timeout_ms = 5000

[queries]
default_limit = 50
max_limit = 500
default_depth = 3
max_depth = 10
max_snippet_bytes = 12000

[mcp]
enabled = true
transport = "stdio"
allow_reindex = false
```

## 14.1 Reglas

- Rutas relativas se resuelven respecto al project root.
- El root debe canonicalizarse.
- La base debe permanecer dentro de `.codekurve` por defecto.
- No aceptar configuración remota.
- No interpretar comandos shell desde config.
- No cargar plugins arbitrarios en `0.1`.
- Validar límites.
- Mostrar errores con ubicación del campo.

---

# 15. Descubrimiento de proyecto

Orden de resolución:

1. `--root`;
2. buscar `.codekurve/config.toml` hacia arriba;
3. detectar `.git`;
4. usar cwd si se pasó `--allow-uninitialized`;
5. error explícito.

No debe escanear directorios hermanos accidentalmente.

## 15.1 Reglas de archivos

- Respetar `.gitignore`.
- Respetar `.ignore`.
- Respetar exclusiones globales si está habilitado.
- No seguir symlinks por defecto.
- Detectar loops.
- Ignorar archivos binarios.
- Ignorar archivos mayores al límite.
- Normalizar separadores.
- Guardar rutas relativas con `/` en la base, incluso en Windows.
- Preservar ruta original para display cuando sea necesario.

## 15.2 Detección binaria

Primera versión:

- extensiones conocidas;
- bytes NUL en una muestra inicial;
- UTF-8 válido o estrategia explícita;
- no intentar indexar contenido desconocido.

Registrar razón de ignore:

```text
gitignore
config_pattern
unsupported_extension
binary
too_large
permission_denied
symlink
parse_error
```

---

# 16. Identidad y hashing

## 16.1 File identity

`file_key`:

```text
BLAKE3(project_id + normalized_relative_path)
```

`content_hash`:

```text
BLAKE3(file_bytes)
```

No depender únicamente de mtime.

## 16.2 Fast change detection

Pipeline:

1. comparar path;
2. comparar size;
3. comparar mtime;
4. si cambió, calcular hash;
5. si hash es igual, actualizar metadata sin reparse;
6. si hash cambió, reindexar.

La verdad final es el hash, no mtime.

## 16.3 Symbol identity

Definir dos conceptos:

### `symbol_id`

Identificador persistido interno.

### `symbol_key`

Identidad lógica determinista:

```text
language
relative_path
symbol_kind
qualified_name
signature_fingerprint
```

Hash:

```text
BLAKE3(canonical_tuple)
```

No incluir líneas porque cambian con frecuencia.

Limitación aceptada en MVP:

- mover un símbolo de archivo puede cambiar su identidad;
- renombrar un símbolo cambia identidad;
- la reconciliación de movimientos puede añadirse después.

## 16.4 Reference identity

Una referencia se identifica por:

```text
source_file
source_span
relationship_kind
target_key_or_unresolved_name
```

---

# 17. Modelo de dominio

## 17.1 LanguageId

```rust
pub enum LanguageId {
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    CSharp,
}
```

No usar strings libres internamente.

## 17.2 SymbolKind

MVP:

```rust
pub enum SymbolKind {
    Module,
    Namespace,
    Class,
    Interface,
    Struct,
    Enum,
    Function,
    Method,
    Constructor,
    Property,
    Field,
    Variable,
    Parameter,
    TypeAlias,
    Import,
    Export,
}
```

Mantener capacidad para:

```text
Controller
Route
Service
Repository
Component
Decorator
```

como tags/framework roles, no necesariamente `SymbolKind`.

## 17.3 RelationshipKind

```rust
pub enum RelationshipKind {
    Defines,
    Contains,
    Imports,
    Exports,
    References,
    Calls,
    Constructs,
    Inherits,
    Implements,
    Overrides,
    UsesType,
    Reads,
    Writes,
}
```

Futuro:

```text
Injects
RegisteredAs
HandlesRoute
Triggers
Publishes
Subscribes
PersistsTo
```

## 17.4 Provenance

```rust
pub enum Provenance {
    Extracted,
    Resolved,
    Heuristic,
}
```

## 17.5 Confidence

```rust
pub enum Confidence {
    Exact,
    High,
    Medium,
    Low,
    Unresolved,
}
```

## 17.6 SourceSpan

```rust
pub struct SourceSpan {
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub start_column: u32,
    pub end_line: u32,
    pub end_column: u32,
}
```

Usar líneas 1-based en interfaces humanas y decidir explícitamente si internamente se guardan 0-based.

---

# 18. Intermediate Representation

Cada analizador produce un `FileAnalysis`.

```rust
pub struct FileAnalysis {
    pub file: AnalyzedFile,
    pub symbols: Vec<ExtractedSymbol>,
    pub relationships: Vec<ExtractedRelationship>,
    pub unresolved: Vec<UnresolvedReference>,
    pub diagnostics: Vec<AnalysisDiagnostic>,
}
```

## 18.1 ExtractedSymbol

Debe incluir:

- local key;
- name;
- qualified name;
- kind;
- language;
- span;
- signature;
- visibility;
- parent symbol;
- tags;
- documentation summary opcional;
- attributes/decorators relevantes;
- is_exported.

## 18.2 ExtractedRelationship

Debe incluir:

- source local key;
- target local/global key si se resolvió;
- unresolved target text si no;
- kind;
- span;
- provenance;
- confidence;
- analyzer;
- reason opcional.

## 18.3 UnresolvedReference

Nunca descartar silenciosamente una referencia.

Guardar:

- origin;
- textual target;
- import context;
- namespace/module context;
- candidate count;
- diagnostic reason.

---

# 19. Arquitectura de analizadores

Trait inicial:

```rust
pub trait LanguageAnalyzer: Send + Sync {
    fn language(&self) -> LanguageId;
    fn supports_path(&self, path: &Path) -> bool;
    fn analyze(&self, input: AnalyzeInput<'_>) -> Result<FileAnalysis, AnalysisError>;
}
```

Puede añadirse:

```rust
pub trait ProjectResolver {
    fn resolve(
        &self,
        project: &ProjectSnapshot,
        analyses: &mut [FileAnalysis],
    ) -> Result<ResolutionReport, ResolutionError>;
}
```

Separar:

1. extracción por archivo;
2. resolución por proyecto;
3. enriquecimiento de framework.

## 19.1 No crear plugins dinámicos todavía

Usar registro estático:

```rust
AnalyzerRegistry
```

El plugin system futuro puede aparecer cuando existan necesidades de equipos diferentes.

---

# 20. TypeScript y JavaScript

## 20.1 Alcance sintáctico del MVP

Extraer:

- imports estáticos;
- exports;
- default exports;
- clases;
- interfaces;
- enums;
- type aliases;
- funciones;
- arrow functions asignadas;
- métodos;
- constructores;
- propiedades;
- `extends`;
- `implements`;
- `new`;
- call expressions;
- member call expressions;
- decorators como metadata;
- módulos y namespaces.

## 20.2 Resolución de módulos

Orden inicial:

1. paths relativos;
2. archivos exactos;
3. extensión implícita:
   - `.ts`;
   - `.tsx`;
   - `.js`;
   - `.jsx`;
4. `index.*`;
5. aliases simples de `tsconfig.json`;
6. paquetes externos se registran como nodos externos, pero no se indexan.

No indexar `node_modules`.

## 20.3 Qualified names

Ejemplos:

```text
src/services/member.service.ts::MemberService
src/services/member.service.ts::MemberService.getEligibility
src/utils.ts::formatDate
```

## 20.4 Calls

Niveles:

### Exact

Llamada local directa resuelta:

```ts
function a() {
  b();
}
```

### High

Método resuelto por variable local/import claro.

### Medium

Member call donde el target se infiere por nombre y contexto.

### Low

Coincidencia de nombre ambigua.

Las herramientas deben permitir:

```text
--min-confidence high
```

## 20.5 Angular futuro, no `0.1.0` base

En fase framework-aware:

- `@Component`;
- `@Injectable`;
- `@Directive`;
- `@Pipe`;
- `@NgModule`;
- standalone components;
- `inject()`;
- constructor injection;
- Router config;
- guards;
- resolvers;
- interceptors;
- inputs;
- outputs;
- template references.

No bloquear el MVP por Angular profundo.

---

# 21. C# y .NET

C# comienza después de que el vertical slice TypeScript sea estable.

## 21.1 Alcance sintáctico

Extraer:

- namespaces;
- classes;
- interfaces;
- structs;
- records;
- enums;
- delegates;
- methods;
- constructors;
- properties;
- fields;
- using directives;
- inheritance;
- interface implementation;
- invocations;
- object creation;
- attributes;
- generic type references.

## 21.2 Resolución inicial

- namespaces;
- same-file types;
- project types por qualified name;
- using directives;
- métodos con coincidencia no ambigua;
- interface implementation declarada.

## 21.3 Limitaciones aceptadas

Tree-sitter no sustituye Roslyn. En MVP:

- overload resolution puede ser parcial;
- dynamic dispatch será aproximado;
- extension methods pueden quedar unresolved;
- generics complejos pueden generar múltiples candidates;
- partial classes requieren combinación;
- generated code puede excluirse.

Toda limitación debe exponerse.

## 21.4 Enriquecimiento .NET futuro

- ASP.NET controllers;
- route attributes;
- minimal APIs;
- middleware;
- `AddScoped`;
- `AddTransient`;
- `AddSingleton`;
- keyed services;
- configuration binding;
- EF Core `DbContext`;
- repositories;
- Azure Functions triggers;
- durable orchestration;
- hosted services.

## 21.5 Adaptador semántico opcional futuro

Un worker basado en Roslyn puede añadirse como opt-in, sin cambiar el modelo de dominio.

CodeKurve seguirá siendo Rust-first:

```text
Rust core
└── optional semantic worker protocol
    └── Roslyn implementation
```

No implementar esto antes de medir las limitaciones reales del análisis en Rust.

---

# 22. Pipeline de indexación completa

```text
Resolve project
    ↓
Load and validate config
    ↓
Open/migrate database
    ↓
Discover files
    ↓
Classify files
    ↓
Read metadata
    ↓
Hash candidates
    ↓
Parse supported files in parallel
    ↓
Produce per-file IR
    ↓
Build project symbol table
    ↓
Resolve imports and references
    ↓
Build relationships
    ↓
Validate graph batch
    ↓
Write transaction
    ↓
Update FTS
    ↓
Persist index run metrics
    ↓
Return report
```

## 22.1 Paralelismo

- Discovery puede ser paralelo moderadamente.
- Hashing puede ser paralelo.
- Parsing debe usar Rayon.
- Resolución puede usar fases paralelas, pero debe ser determinista.
- SQLite usa un writer coordinado.
- No abrir una conexión de escritura por archivo.
- No hacer commits individuales por símbolo.

## 22.2 Backpressure

Usar canales acotados:

```text
discovery → hashing → parsing → resolution → writer
```

Evitar cargar todo el código del repositorio en memoria.

Para MVP se puede analizar por lotes, pero la arquitectura no debe requerir retener todos los source bytes.

## 22.3 Transacciones

Estrategia full index:

1. crear `index_run`;
2. escribir a staging tables o usar versioning por run;
3. validar;
4. promover run;
5. marcar anterior como inactive;
6. limpiar en background o en maintenance.

Alternativa MVP más simple:

- una transacción única por full index para repositorios pequeños/medianos;
- documentar límites;
- evolucionar a staging cuando sea necesario.

Preferencia recomendada: introducir `index_generation` desde el principio para evitar una migración difícil.

---

# 23. Indexación incremental

## 23.1 Eventos

Watcher observa:

- create;
- modify;
- remove;
- rename.

## 23.2 Debounce

Default:

```text
750 ms
```

Los eventos se agrupan por path.

## 23.3 Reconciliación

Los watchers pueden perder eventos. Siempre implementar:

- reconcile al iniciar;
- comando manual `codekurve index`;
- validación por metadata + hash;
- status de archivos pending.

## 23.4 Actualización por archivo

Para archivo modificado:

1. canonicalizar path;
2. verificar que sigue dentro del root;
3. verificar ignores;
4. hash;
5. parse;
6. resolver relaciones locales;
7. identificar dependientes;
8. resolver conjunto afectado;
9. transacción:
   - reemplazar símbolos del archivo;
   - reemplazar relaciones originadas;
   - actualizar referencias entrantes afectadas;
   - actualizar FTS;
   - actualizar metadata.
10. marcar índice fresh.

## 23.5 Eliminación

Al eliminar archivo:

- borrar o soft-delete file;
- borrar símbolos;
- borrar relaciones salientes;
- convertir relaciones entrantes en unresolved cuando aplique;
- re-resolver dependientes.

## 23.6 Rename

MVP puede tratar rename como delete + create.

Futuro:

- detectar hash idéntico;
- preservar identidad;
- remapear paths.

## 23.7 Dependientes

Mantener reverse dependencies por:

- imports;
- references;
- inheritance;
- implementations.

No reanalizar todo el repo por un cambio aislado.

---

# 24. SQLite

## 24.1 Configuración

Al abrir:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
```

Validar soporte FTS5 en `doctor`.

## 24.2 Esquema inicial

### projects

```sql
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    root_path TEXT NOT NULL UNIQUE,
    config_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### files

```sql
CREATE TABLE files (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    relative_path TEXT NOT NULL,
    language TEXT,
    size_bytes INTEGER NOT NULL,
    modified_ns INTEGER,
    content_hash TEXT,
    parse_status TEXT NOT NULL,
    parse_error TEXT,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(project_id, relative_path),
    FOREIGN KEY(project_id) REFERENCES projects(id)
);
```

### symbols

```sql
CREATE TABLE symbols (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    file_id TEXT NOT NULL,
    symbol_key TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    language TEXT NOT NULL,
    visibility TEXT,
    signature TEXT,
    parent_symbol_id TEXT,
    start_byte INTEGER NOT NULL,
    end_byte INTEGER NOT NULL,
    start_line INTEGER NOT NULL,
    start_column INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    end_column INTEGER NOT NULL,
    provenance TEXT NOT NULL,
    confidence TEXT NOT NULL,
    is_exported INTEGER NOT NULL DEFAULT 0,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(project_id, symbol_key),
    FOREIGN KEY(file_id) REFERENCES files(id),
    FOREIGN KEY(parent_symbol_id) REFERENCES symbols(id)
);
```

### relationships

```sql
CREATE TABLE relationships (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    source_symbol_id TEXT NOT NULL,
    target_symbol_id TEXT,
    target_external TEXT,
    kind TEXT NOT NULL,
    provenance TEXT NOT NULL,
    confidence TEXT NOT NULL,
    source_file_id TEXT NOT NULL,
    start_line INTEGER,
    start_column INTEGER,
    reason TEXT,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    FOREIGN KEY(source_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY(target_symbol_id) REFERENCES symbols(id),
    FOREIGN KEY(source_file_id) REFERENCES files(id)
);
```

### unresolved_references

```sql
CREATE TABLE unresolved_references (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    source_symbol_id TEXT,
    source_file_id TEXT NOT NULL,
    relationship_kind TEXT NOT NULL,
    target_text TEXT NOT NULL,
    context_json TEXT,
    candidate_count INTEGER NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    confidence TEXT NOT NULL,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
```

### index_runs

```sql
CREATE TABLE index_runs (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    mode TEXT NOT NULL,
    status TEXT NOT NULL,
    generation INTEGER NOT NULL,
    files_discovered INTEGER NOT NULL DEFAULT 0,
    files_indexed INTEGER NOT NULL DEFAULT 0,
    files_skipped INTEGER NOT NULL DEFAULT 0,
    parse_errors INTEGER NOT NULL DEFAULT 0,
    symbol_count INTEGER NOT NULL DEFAULT 0,
    relationship_count INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    error TEXT
);
```

### diagnostics

```sql
CREATE TABLE diagnostics (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    file_id TEXT,
    code TEXT NOT NULL,
    severity TEXT NOT NULL,
    message TEXT NOT NULL,
    start_line INTEGER,
    start_column INTEGER,
    generation INTEGER NOT NULL,
    created_at TEXT NOT NULL
);
```

## 24.3 FTS5

Índice sobre:

- name;
- qualified name;
- signature;
- kind;
- relative path.

No incluir source completo inicialmente.

## 24.4 Índices SQL

Crear índices para:

```text
symbols(project_id, name)
symbols(project_id, qualified_name)
symbols(file_id)
relationships(source_symbol_id, kind)
relationships(target_symbol_id, kind)
relationships(project_id, kind)
unresolved_references(project_id, target_text)
files(project_id, relative_path)
```

## 24.5 Migraciones

- migraciones numeradas;
- tabla `schema_migrations`;
- forward-only en producción;
- tests desde base vacía;
- tests de upgrade desde cada versión soportada;
- backup antes de migración destructiva;
- `doctor` muestra schema.

---

# 25. Source snippets sin duplicar código

No guardar el source completo por defecto.

Para responder una query:

1. leer metadata del símbolo;
2. leer archivo actual;
3. calcular o verificar hash si es necesario;
4. extraer span con contexto;
5. devolver snippet;
6. si hash no coincide:
   - marcar respuesta stale;
   - no afirmar que el snippet pertenece al índice;
   - solicitar reindex o devolver contenido actual etiquetado.

Configuración futura:

```toml
[index]
store_source = false
```

Si algún ambiente requiere índice completamente portátil, se puede añadir cache opcional cifrado o explícito, pero no en MVP.

---

# 26. Query engine

## 26.1 Search

Entrada:

- query;
- kinds;
- languages;
- path prefix;
- limit;
- offset;
- exact/fuzzy;
- min confidence.

Ranking sugerido:

1. exact qualified name;
2. exact name;
3. prefix name;
4. FTS rank;
5. path relevance;
6. kind preference.

## 26.2 References

Devuelve:

- definitions;
- imports;
- type uses;
- calls;
- reads/writes cuando disponibles;
- unresolved candidates opcionales.

## 26.3 Callers y callees

Filtros:

- depth;
- min confidence;
- include external;
- include tests;
- max nodes;
- max edges.

## 26.4 Trace path

Usar BFS inicialmente.

Parámetros:

```text
from
to
allowed_edge_types
max_depth
max_paths
min_confidence
```

Evitar explosión combinatoria.

## 26.5 Impact analysis

No afirmar que es impacto garantizado.

Nombre correcto:

```text
potential impact
```

Algoritmo inicial:

- reverse graph traversal;
- edge weights;
- max depth;
- exclude containment opcional;
- agrupar por archivo/módulo;
- explicar por qué cada nodo está incluido.

Ejemplo:

```text
IMemberService
  <- implements MemberService
  <- injected by EligibilityController
  <- referenced by eligibility.routes.ts
```

## 26.6 Project overview

Resumen:

- files by language;
- symbols by kind;
- relationships by kind;
- top imported modules;
- high-degree nodes;
- entry point candidates;
- unresolved rate;
- stale files;
- parse errors.

No llamar “arquitectura” a inferencias débiles.

---

# 27. CLI

## 27.1 Comandos

```text
codekurve init
codekurve index
codekurve watch
codekurve status
codekurve doctor
codekurve search
codekurve symbol
codekurve references
codekurve callers
codekurve callees
codekurve implementations
codekurve trace
codekurve impact
codekurve overview
codekurve mcp
codekurve config
codekurve clean
codekurve version
```

## 27.2 Convenciones

Todos los comandos de lectura soportan:

```text
--json
--root
--limit
--offset
```

Cuando aplique:

```text
--depth
--min-confidence
--language
--kind
--path
```

## 27.3 Códigos de salida

```text
0 success
1 general error
2 invalid arguments/config
3 project not initialized
4 index missing
5 index stale beyond policy
6 symbol ambiguous
7 symbol not found
8 database error
9 analysis error
10 security boundary violation
```

## 27.4 Ambigüedad

Si un nombre devuelve múltiples símbolos:

```text
Ambiguous symbol: getEligibility

1. EligibilityService.getEligibility
2. EligibilityApi.getEligibility
3. MockEligibilityService.getEligibility

Use --symbol-id or a qualified name.
```

Nunca seleccionar silenciosamente el primer resultado.

## 27.5 Salida JSON

Debe ser estable y versionada:

```json
{
  "schema_version": 1,
  "project": "sample",
  "result": {},
  "warnings": [],
  "truncated": false
}
```

---

# 28. MCP

## 28.1 Transporte

MVP:

```text
stdio only
```

Razones:

- fácil integración;
- no abre puertos;
- no requiere auth;
- lifecycle controlado por el cliente;
- menor superficie de seguridad.

No implementar HTTP MCP inicialmente.

## 28.2 Herramientas

### `codekurve_project_status`

Devuelve:

- project;
- root;
- index status;
- generation;
- last index;
- pending;
- parse errors;
- schema.

### `codekurve_search_symbols`

Input:

```json
{
  "query": "EligibilityService",
  "kinds": ["class", "interface"],
  "languages": ["typescript"],
  "path_prefix": "src/",
  "limit": 20
}
```

### `codekurve_get_symbol`

Input:

```json
{
  "symbol_id": "...",
  "include_source": true,
  "context_lines": 8
}
```

### `codekurve_find_references`

### `codekurve_find_callers`

### `codekurve_find_callees`

### `codekurve_find_implementations`

### `codekurve_trace_path`

### `codekurve_analyze_impact`

### `codekurve_project_overview`

### `codekurve_doctor`

### `codekurve_reindex`

Deshabilitada por defecto mediante config.

## 28.3 Diseño de respuestas

Las respuestas deben ser:

- compactas;
- estructuradas;
- explicables;
- acotadas;
- con source paths;
- con line ranges;
- con confidence;
- con provenance;
- con stale warning;
- con total count.

No devolver 10,000 nodos.

## 28.4 Guía para agentes

Crear `docs/AGENT_USAGE.md` con reglas:

1. Consultar CodeKurve antes de hacer una exploración amplia.
2. Usar búsqueda textual directa cuando se busca una cadena literal.
3. Verificar source actual antes de editar.
4. No confiar en edges low confidence para cambios críticos.
5. Usar `trace_path` para flujos.
6. Usar `impact` como candidato, no como garantía.
7. Después de cambios grandes, esperar watcher o ejecutar reindex.
8. Si la respuesta dice stale, leer el archivo actual.

---

# 29. Seguridad

## 29.1 Threat model

Amenazas:

- indexar secretos;
- escapar project root;
- symlink traversal;
- path traversal;
- archivos enormes;
- archivos malformados;
- gramáticas con errores;
- consumo excesivo de CPU/RAM;
- base corrupta;
- inyección de contenido en logs;
- output MCP excesivo;
- dependencia comprometida;
- ejecución accidental de código del repositorio;
- lectura de generated artifacts sensibles.

## 29.2 Controles

- no ejecutar código analizado;
- no ejecutar scripts de package manager;
- no cargar config ejecutable;
- no importar módulos del repositorio;
- no ejecutar `npm`, `dotnet`, `cargo` o shells durante indexación;
- canonicalización;
- symlinks off;
- max file size;
- max total files configurable;
- timeout/cancelación;
- memory budgets;
- no network;
- logs estructurados;
- redacción de content;
- dependency audit;
- SBOM;
- checksums de releases;
- firma de binarios futura.

## 29.3 Archivos sensibles

Excluir por defecto, pero permitir que empresa ajuste:

```text
.env
.env.*
secrets.*
*.pfx
*.p12
*.pem
*.key
appsettings.*.json
local.settings.json
credentials*
```

No asumir que toda configuración debe excluirse. Algunos proyectos necesitan analizar `appsettings.json`; la política debe ser configurable.

## 29.4 Network denial

La aplicación no debe depender de un HTTP client.

Añadir prueba/inspección de dependencias para impedir introducción accidental de crates de red sin ADR.

## 29.5 Logs

Default:

```text
info
```

No registrar:

- source completo;
- secrets;
- tokens;
- env values;
- snippets salvo modo debug explícito.

---

# 30. Observabilidad

Usar `tracing`.

Campos:

```text
project_id
run_id
stage
file_count
symbol_count
relationship_count
duration_ms
language
error_code
```

No incluir paths absolutos en logs compartidos si puede evitarse.

## 30.1 Timing stages

Medir:

- config;
- discovery;
- metadata;
- hashing;
- parsing;
- resolution;
- database;
- FTS;
- total.

## 30.2 Status

`codekurve status`:

```text
Project: sample
Root: /repo/sample
Index: fresh
Generation: 42
Files: 1,284
Symbols: 18,902
Relationships: 47,315
Unresolved: 1,205
Parse errors: 3
Last full index: 2026-07-21T...
Pending changes: 0
Database: .codekurve/index.db
Network features: none
```

---

# 31. Error model

## 31.1 Typed errors

Cada crate define errores propios y la aplicación los transforma.

Ejemplos:

```text
ConfigError
ProjectResolutionError
DiscoveryError
AnalysisError
ResolutionError
StoreError
QueryError
McpError
SecurityError
```

## 31.2 Error codes

Definir códigos estables:

```text
CK_CONFIG_INVALID
CK_PROJECT_NOT_FOUND
CK_PATH_OUTSIDE_ROOT
CK_FILE_TOO_LARGE
CK_LANGUAGE_UNSUPPORTED
CK_PARSE_FAILED
CK_DB_MIGRATION_FAILED
CK_DB_CORRUPT
CK_FTS_UNAVAILABLE
CK_SYMBOL_NOT_FOUND
CK_SYMBOL_AMBIGUOUS
CK_QUERY_LIMIT_EXCEEDED
CK_INDEX_STALE
```

## 31.3 Partial success

Una falla de parsing en un archivo no debe cancelar todo el índice, salvo:

- database failure;
- migration failure;
- invariant violation;
- cancellation;
- policy threshold exceeded.

Reportar:

```text
completed_with_errors
```

---

# 32. Cancelación

Indexación debe admitir Ctrl+C.

Requisitos:

- señal de cancelación;
- detener discovery;
- no iniciar nuevos parse jobs;
- terminar o cancelar jobs;
- rollback de transacción incompleta;
- registrar run como cancelled;
- conservar índice anterior válido.

---

# 33. Performance budgets

Estos son objetivos iniciales, no promesas. Deben medirse en hardware documentado.

## 33.1 Fixture small

```text
100 files
10k LOC
```

Objetivos:

- cold index < 1 segundo;
- symbol search p95 < 25 ms;
- callers p95 < 50 ms.

## 33.2 Fixture medium

```text
1,000 files
100k–250k LOC
```

Objetivos:

- cold index < 8 segundos;
- one-file update < 500 ms después de debounce;
- search p95 < 50 ms;
- traversal depth 3 < 150 ms;
- peak memory < 750 MB.

## 33.3 Fixture large

```text
10,000 files
1M+ LOC
```

Objetivos iniciales:

- completar sin OOM en máquina de 8 GB;
- cold index < 90 segundos;
- one-file update < 1 segundo después de debounce;
- search p95 < 100 ms;
- bounded traversal.

## 33.4 Reglas

- documentar CPU, RAM, OS y storage;
- correr mínimo 5 veces;
- reportar median y p95 cuando aplique;
- separar cold cache y warm cache;
- no comparar con herramientas externas sin metodología reproducible;
- guardar benchmark baselines.

---

# 34. Calidad y precisión

## 34.1 Métricas

- parse success rate;
- symbol precision;
- symbol recall;
- import resolution rate;
- call edge precision;
- call edge recall;
- unresolved rate;
- false-positive impact nodes;
- stale response rate.

## 34.2 Golden fixtures

Cada fixture debe tener expected graph:

```text
fixtures/typescript/basic/
├── src/
├── expected_symbols.json
├── expected_relationships.json
└── README.md
```

## 34.3 Casos TypeScript

- relative imports;
- aliases;
- re-exports;
- default exports;
- classes;
- interfaces;
- overloaded names;
- methods;
- arrow functions;
- nested functions;
- inheritance;
- interface implementation;
- cycles;
- barrel files;
- TSX;
- decorators;
- unresolved package imports.

## 34.4 Casos C#

- namespace;
- file-scoped namespace;
- classes;
- interfaces;
- records;
- partial classes;
- methods;
- overloads;
- inheritance;
- interface implementation;
- attributes;
- extension methods;
- generics;
- `using`;
- aliases;
- invocation;
- object creation.

---

# 35. Testing

## 35.1 Unit tests

- path normalization;
- config;
- hashing;
- symbol keys;
- source spans;
- ranking;
- edge filtering;
- migrations;
- error mapping.

## 35.2 Parser tests

- one construct per fixture;
- malformed code;
- comments;
- unicode;
- CRLF;
- empty file;
- large file threshold.

## 35.3 Integration tests

- init;
- full index;
- query;
- modify file;
- incremental update;
- delete;
- rename;
- stale detection;
- database rebuild;
- MCP calls.

## 35.4 Property tests

- path normalization invariants;
- IDs deterministic;
- no path escapes;
- query limits;
- serialization roundtrip.

## 35.5 Snapshot tests

Usar para:

- CLI human output;
- JSON output;
- MCP results;
- diagnostics;
- graph fixtures.

Revisar snapshots, no actualizarlos automáticamente sin inspección.

## 35.6 Fault injection

Simular:

- permission denied;
- DB locked;
- corrupt DB;
- parser panic boundary;
- cancellation;
- file changed during read;
- watcher event storms;
- symlink loop;
- invalid UTF-8.

---

# 36. Benchmarks y comparación interna

No comenzar comparando marketing.

Primero crear baseline:

## 36.1 Tareas

1. Buscar símbolo.
2. Encontrar callers.
3. Trazar endpoint a repository.
4. Listar implementaciones.
5. Estimar impacto.
6. Obtener overview.

## 36.2 Comparación de agente

En un repositorio de prueba:

- agente con CodeKurve;
- agente sin CodeKurve;
- misma pregunta;
- mismo modelo;
- mismas herramientas restantes;
- múltiples runs.

Medir:

- wall time;
- tool calls;
- files read;
- tokens;
- answer correctness;
- missed dependencies.

No usar estos resultados como verdad absoluta.

---

# 37. CI

## 37.1 Pull request checks

- format;
- clippy;
- tests;
- doc tests;
- MSRV futuro;
- dependency licenses;
- dependency vulnerabilities;
- deny network dependency policy;
- migration tests;
- fixture validation;
- benchmark smoke, no benchmark completo.

## 37.2 Platforms

- Ubuntu;
- Windows;
- macOS.

## 37.3 Architectures

Inicial:

- x86_64 Linux;
- x86_64 Windows;
- x86_64 macOS;
- aarch64 macOS.

Futuro:

- aarch64 Linux.

## 37.4 Reproducibility

- lockfile;
- release profile;
- checksums;
- build metadata;
- version command.

---

# 38. Release profile

Propuesta:

```toml
[profile.release]
lto = "thin"
codegen-units = 1
panic = "abort"
strip = "symbols"
```

No decidir `opt-level = 3` vs `s` sin medir.

Crear también:

```toml
[profile.bench]
debug = true
```

Evaluar:

- binary size;
- startup;
- parsing throughput;
- debugging.

---

# 39. Distribución empresarial

## 39.1 MVP

Entregar:

- binario;
- SHA-256 checksum;
- SBOM;
- third-party licenses;
- versión;
- documentación de instalación;
- documentación de desinstalación;
- ubicación de datos;
- procedimiento de limpieza.

## 39.2 Instalación

No requerir admin si es posible.

Ejemplo:

```text
tools/codekurve/<version>/codekurve.exe
```

## 39.3 Desinstalación

- borrar binario;
- borrar config MCP;
- opcionalmente borrar `.codekurve`;
- no dejar servicios;
- no dejar tasks;
- no dejar puertos.

## 39.4 Actualización

Inicialmente manual y controlada.

No auto-update.

---

# 40. Licencias e IP

Antes de definir licencia pública:

1. aclarar propiedad intelectual;
2. revisar política del empleador;
3. determinar si el proyecto se desarrolla como interno, personal u open source;
4. revisar licencias de dependencias;
5. generar notices.

Hasta resolverlo:

- no publicar;
- no añadir una licencia open source por defecto;
- usar un aviso interno;
- no copiar código de Graphify o CodeGraph;
- usar ideas y patrones generales, no implementación propietaria;
- documentar inspiración sin asumir compatibilidad.

---

# 41. Documentación obligatoria

## README.md

- qué es;
- qué no es;
- quickstart;
- estado experimental;
- supported languages;
- commands;
- security promise;
- limitations.

## ARCHITECTURE.md

- components;
- data flow;
- concurrency;
- boundaries;
- decisions.

## DATA_MODEL.md

- schema;
- IDs;
- edges;
- confidence;
- provenance.

## SECURITY_MODEL.md

- threat model;
- no-network;
- ignored files;
- storage;
- update process.

## PERFORMANCE.md

- benchmark method;
- baselines;
- known bottlenecks.

## MCP.md

- setup;
- tools;
- schemas;
- agent guidance.

## ROADMAP.md

- phases;
- status;
- exit criteria.

## CONTRIBUTING.md

- commands;
- tests;
- ADRs;
- dependency policy;
- commit expectations.

---

# 42. ADRs iniciales

Crear:

```text
0001-rust-first.md
0002-sqlite-storage.md
0003-tree-sitter-parsing.md
0004-stdio-only-mcp.md
0005-no-network-no-telemetry.md
0006-source-not-stored-by-default.md
0007-confidence-and-provenance.md
0008-single-writer-sqlite.md
0009-typescript-first.md
0010-static-analyzer-registry.md
```

Cada ADR:

```text
Context
Decision
Alternatives
Consequences
Status
```

---

# 43. Roadmap por fases

# Fase 0 — Gobernanza y scaffold

## Objetivo

Crear un repositorio que compile, tenga límites claros y pueda evolucionar sin deuda inmediata.

## Tareas

- crear workspace;
- configurar toolchain;
- crear crates iniciales;
- configurar clippy;
- configurar rustfmt;
- añadir CI;
- crear docs;
- crear ADRs iniciales;
- definir licencia temporal interna;
- configurar dependency policy;
- implementar `codekurve version`;
- implementar logging;
- implementar error envelope;
- agregar tests smoke.

## Exit criteria

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo run -- version
```

Todo pasa en Windows, macOS y Linux CI.

---

# Fase 1 — Vertical slice mínimo

## Objetivo

Indexar un archivo TypeScript y buscar un símbolo desde SQLite.

## Scope

- `codekurve init`;
- config;
- project root;
- one language;
- one parser;
- classes/functions;
- SQLite;
- FTS;
- `index`;
- `search`;
- `symbol`.

## Flujo de demo

```bash
codekurve init fixtures/typescript/basic
codekurve index --root fixtures/typescript/basic
codekurve search MemberService --root fixtures/typescript/basic
codekurve symbol MemberService --root fixtures/typescript/basic
```

## Exit criteria

- símbolo correcto;
- path y span correctos;
- snippet actual;
- DB descartable;
- tests golden;
- no network dependencies.

---

# Fase 2 — Grafo TypeScript

## Objetivo

Construir relaciones útiles.

## Scope

- imports;
- exports;
- contains;
- extends;
- implements;
- calls;
- constructs;
- references;
- unresolved;
- qualified names;
- ambiguity handling.

## CLI

- references;
- callers;
- callees;
- implementations;
- trace;
- impact.

## Exit criteria

- fixtures complejos;
- confidence/provenance;
- path tracing;
- bounded impact;
- documented limitations.

---

# Fase 3 — Incremental y watcher

## Objetivo

Mantener índice fresh.

## Scope

- file metadata;
- BLAKE3;
- watcher;
- debounce;
- reconcile on start;
- create/update/delete;
- transactions;
- pending status;
- stale warning.

## Exit criteria

- modificar un archivo no ejecuta full reindex;
- file delete limpia graph;
- event storm se agrupa;
- Ctrl+C seguro;
- previous index preserved on failure.

---

# Fase 4 — MCP

## Objetivo

Permitir que Claude Code y Codex consulten el índice.

## Scope

- `stdio`;
- tools;
- schemas;
- result caps;
- JSON tests;
- installation docs;
- agent instructions.

## Exit criteria

- conexión real con al menos un cliente;
- tool calls reproducibles;
- no stdout logs que rompan protocolo;
- logs van a stderr;
- bounded outputs;
- stale state visible.

---

# Fase 5 — C#

## Objetivo

Paridad básica con TypeScript.

## Scope

- parser;
- symbols;
- using;
- inheritance;
- implementation;
- calls;
- constructs;
- attributes;
- namespaces;
- fixtures.

## Exit criteria

- proyecto C# de prueba;
- cross-file resolution;
- limitations published;
- no regresión TypeScript.

---

# Fase 6 — Hardening empresarial

## Objetivo

Preparar piloto interno.

## Scope

- security model;
- SBOM;
- licenses;
- checksums;
- release binaries;
- cleanup;
- config policies;
- max resource controls;
- dependency audit;
- reproducible benchmark report;
- installation package interno.

## Exit criteria

- revisión interna;
- no network;
- documented data paths;
- threat model approved;
- pilot repository selected.

---

# Fase 7 — Angular y .NET aware

## Angular

- components;
- services;
- inject;
- constructor DI;
- routes;
- guards;
- interceptors;
- standalone imports.

## .NET

- controllers;
- minimal APIs;
- DI registrations;
- Azure Functions;
- middleware;
- EF Core contexts.

## Exit criteria

- end-to-end path desde route a data layer en fixtures;
- cada framework edge tiene provenance;
- heuristics claramente marcadas.

---

# Fase 8 — Piloto y evaluación

## Selección

Elegir:

- un repo TypeScript/Angular mediano;
- un repo .NET mediano;
- un flujo real difícil.

## Métricas

- index time;
- incremental time;
- query latency;
- memory;
- precision;
- agent tool calls;
- files read;
- developer satisfaction;
- bugs encontrados.

## Decisión

- continuar;
- ajustar;
- detener;
- integrar en más equipos.

---

# 44. Backlog priorizado

## P0

- scaffold;
- no-network;
- config;
- project resolution;
- discovery;
- hashing;
- TypeScript parser;
- SQLite;
- FTS;
- search;
- symbol;
- tests.

## P1

- imports;
- references;
- calls;
- callers/callees;
- trace;
- impact;
- watcher;
- MCP;
- C#.

## P2

- Angular;
- ASP.NET;
- DI;
- routes;
- benchmarks;
- enterprise packaging.

## P3

- semantic workers;
- multi-repo;
- PR impact;
- visualization;
- policies;
- export.

---

# 45. Primeros 25 issues

1. Initialize Rust workspace.
2. Add CI matrix.
3. Define domain types.
4. Add typed error model.
5. Implement project root resolution.
6. Implement config loading.
7. Implement safe path normalization.
8. Implement file discovery with ignore rules.
9. Implement language detection.
10. Implement BLAKE3 hashing.
11. Create SQLite migration framework.
12. Create initial schema.
13. Add FTS capability check.
14. Implement TypeScript parser adapter.
15. Extract classes and functions.
16. Persist files and symbols.
17. Implement full index command.
18. Implement search command.
19. Implement symbol command and live snippet.
20. Add golden TypeScript fixture.
21. Add index report metrics.
22. Add doctor command.
23. Add cancellation.
24. Add security tests for path escape/symlink.
25. Document vertical slice.

No issue debe combinar cinco subsistemas.

---

# 46. Orden sugerido de commits

1. `chore: initialize Rust workspace`
2. `docs: add project charter and ADRs`
3. `ci: add format clippy and test matrix`
4. `feat(core): define project and language domain types`
5. `feat(config): load and validate project configuration`
6. `feat(discovery): scan files with ignore rules`
7. `feat(core): add stable file hashing`
8. `feat(store): add SQLite migrations`
9. `feat(store): persist projects and files`
10. `feat(analysis): add TypeScript parser`
11. `feat(analysis): extract symbols`
12. `feat(store): persist and search symbols`
13. `feat(cli): add init and index commands`
14. `feat(cli): add search and symbol commands`
15. `test: add TypeScript golden fixtures`
16. `feat(cli): add doctor diagnostics`
17. `perf: add initial indexing benchmark`
18. `docs: document vertical slice limitations`

No forzar exactamente esta secuencia si una dependencia técnica requiere ajuste, pero mantener commits pequeños.

---

# 47. Definition of Done general

Una tarea no está terminada hasta que:

- código compila;
- tests pasan;
- clippy sin warnings;
- format correcto;
- errores manejados;
- docs actualizadas;
- no rompe JSON;
- no rompe MCP;
- no añade network;
- no viola project root;
- tiene test positivo;
- tiene test negativo;
- tiene logging útil;
- tiene changelog cuando es user-facing.

---

# 48. Convenciones de código

## 48.1 Rust

- `#![forbid(unsafe_code)]` inicialmente.
- No `unwrap()` en runtime production.
- `expect()` solo para invariants con mensaje.
- errores tipados en librerías;
- `anyhow` solo en binary boundary;
- evitar clones grandes;
- usar newtypes para IDs;
- documentar APIs públicas;
- mantener funciones pequeñas;
- no usar macros complejas sin beneficio.

## 48.2 SQL

- queries en módulos dedicados;
- parámetros bind;
- no concatenar input;
- transacciones explícitas;
- EXPLAIN para queries críticas;
- migraciones revisadas.

## 48.3 JSON

- snake_case;
- schema version;
- enums como strings estables;
- no exponer paths absolutos por defecto;
- no cambiar campos sin versionar.

---

# 49. Concurrency model

## 49.1 Threading

```text
Main/Tokio runtime
├── CLI or MCP lifecycle
├── cancellation
├── watcher coordination
└── query handling

Rayon pool
├── hashing
└── parsing

SQLite writer
└── serialized transactions
```

## 49.2 Read queries

SQLite puede atender reads concurrentes con WAL, pero:

- usar pool pequeño o conexiones por request controladas;
- medir;
- no crear cientos de conexiones;
- busy timeout.

## 49.3 Parsing

- parser por worker;
- no compartir mutable parser entre threads;
- grammar registry inmutable;
- limitar workers por config;
- `0` significa auto.

## 49.4 Memory

- leer archivos por lotes;
- liberar source bytes después de parse;
- no guardar AST completo de todos los archivos;
- persistir IR compacta;
- evitar strings duplicadas cuando sea costoso;
- medir antes de introducir interning complejo.

---

# 50. Graph traversal

## 50.1 MVP

Implementar en Rust usando adjacency lists cargadas desde SQL para el subgrafo relevante.

No añadir `petgraph` automáticamente.

Evaluar:

- implementación simple;
- petgraph;
- recursive CTE.

## 50.2 Regla

Escoger mediante benchmark.

## 50.3 Límites

- depth;
- nodes;
- edges;
- time budget;
- cancellation.

Respuesta truncada debe decir:

```text
truncated: true
reason: max_nodes
```

---

# 51. Ranking de impacto

Propuesta inicial de pesos:

```text
implements      1.0
inherits        1.0
calls           0.9
constructs      0.9
imports         0.6
uses_type       0.7
references      0.5
contains        0.2
```

No presentar score como probabilidad.

Score sirve para orden.

Incluir explicación:

```text
Included because:
A implements B
C calls A.method
D imports C
```

---

# 52. Entry point detection futura

TypeScript:

- `main.ts`;
- route files;
- exported handler;
- CLI entry;
- package bin;
- test entry.

C#:

- `Program.cs`;
- controllers;
- function triggers;
- hosted services;
- public API methods.

Marcar como candidates.

---

# 53. Qué no debe hacer el agente durante la implementación

- copiar Graphify;
- copiar CodeGraph;
- introducir una UI primero;
- añadir Neo4j;
- añadir embeddings;
- añadir un LLM;
- soportar muchos lenguajes superficialmente;
- usar regex como parser principal;
- prometer semantic correctness;
- almacenar todo el source sin discusión;
- abrir un puerto;
- crear un daemon permanente antes del watcher básico;
- agregar auto-update;
- agregar telemetría;
- implementar remote sync;
- ignorar Windows;
- asumir paths Unix;
- optimizar sin benchmark;
- hacer un mega-commit.

---

# 54. Riesgos principales

## Riesgo 1: precisión de calls

Mitigación:

- confidence;
- provenance;
- unresolved;
- golden tests;
- semantic adapters futuros.

## Riesgo 2: grafo stale

Mitigación:

- watcher;
- reconcile;
- hash;
- stale banners;
- manual index.

## Riesgo 3: scope creep

Mitigación:

- no objetivos;
- phases;
- exit criteria;
- P0/P1.

## Riesgo 4: aprobación empresarial

Mitigación:

- no network;
- SBOM;
- licenses;
- local storage;
- threat model;
- small dependency surface.

## Riesgo 5: rendimiento SQLite

Mitigación:

- WAL;
- indexes;
- batch transactions;
- query plans;
- generation model;
- benchmark.

## Riesgo 6: mantenimiento de grammars

Mitigación:

- version pinning;
- compatibility tests;
- upgrade process;
- fixture suite.

## Riesgo 7: agentes ignoran MCP

Mitigación:

- AGENTS.md;
- CLAUDE.md section;
- tool descriptions claras;
- query-first guidance;
- benchmarks reales.

## Riesgo 8: IP

Mitigación:

- proyecto nuevo;
- no copiar;
- revisar propiedad;
- no publicar prematuramente.

---

# 55. Prompt maestro para iniciar implementación

Copiar este bloque en Codex o Claude Code dentro de un repositorio vacío:

```text
You are the lead engineer responsible for starting CodeKurve.

Read CODEKURVE_MASTER_PLAN.md completely before changing files.

CodeKurve is a new Rust-first, local-only code graph and static-analysis indexer.
It must be auditable for enterprise use. It must not use network access,
telemetry, cloud services, embeddings, vector databases, or LLM inference.

Your immediate scope is Phase 0 only, followed by the smallest vertical slice
from Phase 1. Do not attempt the full roadmap.

Required first actions:

1. Inspect the repository and report its current state.
2. Create a short implementation plan mapped to Phase 0.
3. Initialize a Rust workspace with these initial crates:
   - crates/codekurve
   - crates/codekurve-core
   - crates/codekurve-analysis
   - crates/codekurve-store
   - crates/codekurve-mcp
4. Add rust-toolchain.toml using stable Rust with rustfmt and clippy.
5. Add workspace lint configuration.
6. Add README.md, CHANGELOG.md, SECURITY.md, CONTRIBUTING.md.
7. Add docs/ARCHITECTURE.md, docs/DATA_MODEL.md,
   docs/SECURITY_MODEL.md, docs/PERFORMANCE.md, docs/MCP.md,
   docs/ROADMAP.md, and docs/adr/.
8. Add ADRs 0001 through 0010 listed in the master plan.
9. Add a cross-platform CI workflow for format, clippy, and tests.
10. Implement only a minimal `codekurve version` command.
11. Add tests for the version command.
12. Run formatting, clippy, and tests.
13. Summarize:
    - files created;
    - architecture established;
    - dependencies added and why;
    - commands executed;
    - test results;
    - remaining Phase 0 work.

Constraints:

- Code and repository documentation must be in English.
- Do not add a network-capable dependency.
- Do not add Tree-sitter, SQLite, MCP, or watcher implementation until the
  scaffold is stable, unless the next Phase 0/1 task explicitly needs it.
- Do not use unsafe code.
- Do not use unwrap in production paths.
- Keep commits and changes small.
- Do not claim benchmarks that were not run.
- Do not add an open-source license until project ownership is clarified.
- If a choice is unclear, prefer the smallest design compatible with the plan.

After Phase 0 passes all checks, stop and provide a proposal for the first
Phase 1 vertical-slice pull request. Do not implement Phase 2.
```

---

# 56. Prompt para el segundo ciclo

```text
Continue CodeKurve from the completed Phase 0 scaffold.

Read CODEKURVE_MASTER_PLAN.md and all ADRs. Inspect the current repository and
verify that Phase 0 checks pass before making changes.

Implement the smallest Phase 1 vertical slice:

- initialize a project;
- resolve the project root safely;
- load `.codekurve/config.toml`;
- discover TypeScript files while respecting `.gitignore`;
- parse one fixture using Tree-sitter;
- extract class and top-level function symbols;
- persist projects, files, and symbols in SQLite;
- create a symbol FTS index;
- implement:
  - `codekurve init`
  - `codekurve index`
  - `codekurve search`
  - `codekurve symbol`
- return live source snippets by reading the current file and checking staleness;
- add golden fixtures and integration tests;
- add `codekurve doctor` checks for SQLite, FTS5, project root, and config.

Do not implement imports, calls, MCP, watcher, C#, Angular, or impact analysis
in this change.

Before coding:
1. propose the exact file changes;
2. identify dependencies and licenses;
3. identify schema migration 0001;
4. define acceptance tests.

During coding:
- preserve crate boundaries;
- use typed errors;
- keep SQL parameterized;
- make index writes transactional;
- avoid storing complete source;
- keep output JSON versioned;
- do not access the network.

After coding:
- run fmt, clippy, tests;
- run the demo against the fixture;
- include real command output;
- update docs and changelog;
- report remaining limitations.
```

---

# 57. AGENTS.md inicial sugerido

```markdown
# CodeKurve Agent Instructions

Read `CODEKURVE_MASTER_PLAN.md` before significant work.

## Non-negotiable constraints

- Rust-first.
- Local-only.
- No network.
- No telemetry.
- No cloud.
- No embeddings.
- No LLM-based graph construction.
- SQLite storage.
- MCP over stdio only for the MVP.
- Do not store full source by default.
- Respect project-root boundaries and ignore rules.
- Every relationship must expose provenance and confidence.
- Keep query results bounded.
- Do not claim semantic certainty when analysis is heuristic.
- Do not add a dependency without documenting purpose and license.
- Do not add `unsafe` without an approved ADR.
- Do not use `unwrap` in production paths.
- Do not broaden language support before TypeScript and C# are useful.

## Required checks

Run before completing work:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Update:

- `CHANGELOG.md`
- relevant docs
- ADRs when architectural decisions change
- fixtures when behavior changes

## Work style

- Make small, reviewable changes.
- Preserve a compiling workspace.
- Prefer vertical slices.
- Add negative tests.
- Report real command results.
- Stop at the requested phase.
```

---

# 58. Criterio para decidir si Rust sigue siendo correcto

Después del vertical slice, medir:

- parser throughput;
- DB throughput;
- memory;
- complexity;
- cross-platform build;
- developer velocity.

No abandonar Rust solo porque el primer parser sea difícil.

No insistir dogmáticamente si:

- la precisión requerida obliga a un compiler service;
- la integración empresarial requiere otro runtime;
- el costo supera el beneficio.

La arquitectura permite workers opcionales sin abandonar el Rust core.

---

# 59. Resultado esperado del primer mes

Al final del primer ciclo razonable de desarrollo, CodeKurve debería:

- compilar en tres OS;
- inicializar proyecto;
- indexar TypeScript;
- extraer símbolos;
- buscar;
- mostrar source;
- detectar staleness;
- usar SQLite + FTS;
- tener tests;
- tener CI;
- tener threat model;
- no usar red.

Todavía no necesita:

- resolver todos los calls;
- MCP completo;
- C#;
- Angular;
- UI.

Ese resultado ya permite validar:

- experiencia CLI;
- storage;
- parsing;
- performance;
- seguridad;
- mantenibilidad.

---

# 60. Decisión final de arranque

Comenzar CodeKurve como proyecto nuevo.

Tecnología inicial:

```text
Rust
Cargo workspace
Tree-sitter
SQLite/FTS5
BLAKE3
ignore
Rayon
notify
MCP stdio
```

Orden:

```text
scaffold
→ project discovery
→ TypeScript symbols
→ SQLite/FTS
→ CLI queries
→ relationships
→ incremental index
→ MCP
→ C#
→ frameworks
→ enterprise pilot
```

Regla central:

> Build the smallest trustworthy code graph that is already useful, then improve precision and coverage from measured needs.

