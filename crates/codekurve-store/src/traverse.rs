//! Bounded graph traversal over `relationships` edges: `load_adjacency`
//! builds an in-memory `HashMap` from one SQL query, then `bfs` is a
//! hand-rolled breadth-first search — no petgraph (design §50.1). `trace`
//! (a target is given) finds the shortest path within a depth/node/edge/time
//! budget; `impact` (no target) explores everything reachable within the
//! same budget. Every cap that fires sets `truncated` + a reason — nothing
//! is ever silently incomplete (§26.4/§50.3).

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

use codekurve_core::{Confidence, RelationshipKind};
use rusqlite::{params, Connection};

use crate::error::Result;
use crate::repo::confidence_rank;

/// One edge out of (or, under `load_adjacency(reverse = true)`, into) a node
/// — a display-model string triple, matching `StoredSymbol`/
/// `StoredRelationship`'s convention (`repo.rs`) rather than re-parsing back
/// into the `RelationshipKind`/`Confidence`/`Provenance` enums.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub neighbor_symbol_id: String,
    pub kind: String,
    pub confidence: String,
    pub provenance: String,
}

/// Load every in-project relationship edge as an adjacency list. Edges with
/// no `target_symbol_id` (external/unresolved targets) are dead ends — not
/// traversable — and excluded. `reverse = false` keys by source (walk
/// forward, for `trace`); `reverse = true` keys by target (walk backward,
/// for `impact`).
pub fn load_adjacency(
    conn: &Connection,
    project_id: &str,
    reverse: bool,
) -> Result<HashMap<String, Vec<Edge>>> {
    let mut stmt = conn.prepare(
        "SELECT source_symbol_id, target_symbol_id, kind, confidence, provenance
         FROM relationships
         WHERE project_id = ?1 AND target_symbol_id IS NOT NULL
         ORDER BY rowid",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut adjacency: HashMap<String, Vec<Edge>> = HashMap::new();
    for row in rows {
        let (source, target, kind, confidence, provenance) = row?;
        let (node, neighbor) = if reverse {
            (target, source)
        } else {
            (source, target)
        };
        adjacency.entry(node).or_default().push(Edge {
            neighbor_symbol_id: neighbor,
            kind,
            confidence,
            provenance,
        });
    }
    Ok(adjacency)
}

/// Why a bounded BFS stopped before exhausting the reachable graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruncationReason {
    MaxDepth,
    MaxNodes,
    MaxEdges,
    MaxDuration,
}

/// One node discovered during BFS: the edge (and predecessor) that first
/// reached it. `via`/`predecessor` are `None` only for the start node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    pub symbol_id: String,
    pub depth: u32,
    pub via: Option<Edge>,
    pub predecessor: Option<String>,
}

/// Caps a single BFS run must respect.
#[derive(Debug, Clone, Copy)]
pub struct BfsCaps {
    pub max_depth: u32,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_duration: Duration,
}

/// Outcome of one BFS run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BfsOutcome {
    /// Every node reached, in BFS (shortest-hop) order; always includes
    /// `start` at depth 0.
    pub reached: Vec<Reached>,
    /// For `trace` (`target` supplied): the edge sequence from `start` to
    /// `target`, in order, if found within the caps. Always `None` for
    /// `impact` (`target: None` — there is no single target to path to).
    pub path: Option<Vec<Edge>>,
    pub truncated: bool,
    pub truncated_reason: Option<TruncationReason>,
}

/// Bounded BFS over `adjacency` (as built by `load_adjacency`). `target`
/// requests a shortest path (`trace`, returns early once found); `None`
/// explores everything reachable up to the caps (`impact`).
/// `allowed_kinds`/`min_confidence`, when given, filter which edges may be
/// followed at all (§26.4/§27.2).
pub fn bfs(
    adjacency: &HashMap<String, Vec<Edge>>,
    start: &str,
    target: Option<&str>,
    caps: &BfsCaps,
    allowed_kinds: Option<&[RelationshipKind]>,
    min_confidence: Option<Confidence>,
) -> BfsOutcome {
    bfs_core(
        &mut |node, _queue| adjacency.get(node).cloned().unwrap_or_default(),
        start,
        target,
        caps,
        allowed_kinds,
        min_confidence,
    )
}

/// Edges read from SQLite on demand. The search asks for one node's edges;
/// the load is widened to the nodes already waiting in the BFS queue, so a
/// whole frontier costs one `IN (...)` query instead of one query per node —
/// and nothing outside what the search touches is ever read (the old path
/// loaded every edge of the project, ~110 ms per call at 200k edges).
struct LazyEdges<'a> {
    conn: &'a Connection,
    project_id: &'a str,
    reverse: bool,
    cache: HashMap<String, Vec<Edge>>,
    requested: HashSet<String>,
    edges_loaded: usize,
    error: Option<crate::error::Error>,
}

/// Nodes fetched per query (well under SQLite's variable limit).
const FRONTIER_BATCH: usize = 500;

impl<'a> LazyEdges<'a> {
    fn new(conn: &'a Connection, project_id: &'a str, reverse: bool) -> Self {
        Self {
            conn,
            project_id,
            reverse,
            cache: HashMap::new(),
            requested: HashSet::new(),
            edges_loaded: 0,
            error: None,
        }
    }

    fn edges_of(&mut self, node: &str, queue: &VecDeque<String>) -> Vec<Edge> {
        if !self.requested.contains(node) && self.error.is_none() {
            let mut batch: Vec<&str> = vec![node];
            batch.extend(
                queue
                    .iter()
                    .map(String::as_str)
                    .filter(|n| !self.requested.contains(*n))
                    .take(FRONTIER_BATCH - 1),
            );
            let batch: Vec<String> = batch.into_iter().map(str::to_string).collect();
            if let Err(e) = self.load(&batch) {
                self.error = Some(e);
            }
        }
        self.cache.get(node).cloned().unwrap_or_default()
    }

    /// One query for `nodes`. `+project_id` keeps the planner off the
    /// project-wide index (it would scan every edge of the project once per
    /// batch) and on the per-symbol `source_kind`/`target_kind` index.
    /// Per-node edge order matches a full table scan
    /// (`ORDER BY rowid`), so BFS tie-breaking is identical to the in-memory
    /// path.
    fn load(&mut self, nodes: &[String]) -> Result<()> {
        let (node_col, other_col) = if self.reverse {
            ("target_symbol_id", "source_symbol_id")
        } else {
            ("source_symbol_id", "target_symbol_id")
        };
        let placeholders = vec!["?"; nodes.len()].join(",");
        let sql = format!(
            "SELECT {node_col}, {other_col}, kind, confidence, provenance
             FROM relationships
             WHERE +project_id = ? AND target_symbol_id IS NOT NULL
               AND {node_col} IN ({placeholders})
             ORDER BY rowid"
        );
        let mut binds: Vec<&str> = vec![self.project_id];
        binds.extend(nodes.iter().map(String::as_str));
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(binds), |row| {
            Ok((
                row.get::<_, String>(0)?,
                Edge {
                    neighbor_symbol_id: row.get(1)?,
                    kind: row.get(2)?,
                    confidence: row.get(3)?,
                    provenance: row.get(4)?,
                },
            ))
        })?;
        for node in nodes {
            self.requested.insert(node.clone());
        }
        for row in rows {
            let (node, edge) = row?;
            self.edges_loaded += 1;
            self.cache.entry(node).or_default().push(edge);
        }
        Ok(())
    }

    #[cfg(test)]
    fn loaded_edge_count(&self) -> usize {
        self.edges_loaded
    }

    fn finish(self) -> Result<()> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// [`bfs`] over the database: same result as `bfs(&load_adjacency(..), ..)`,
/// reading only the edges around the nodes the search visits.
#[allow(clippy::too_many_arguments)]
pub fn bfs_db(
    conn: &Connection,
    project_id: &str,
    reverse: bool,
    start: &str,
    target: Option<&str>,
    caps: &BfsCaps,
    allowed_kinds: Option<&[RelationshipKind]>,
    min_confidence: Option<Confidence>,
) -> Result<BfsOutcome> {
    let mut lazy = LazyEdges::new(conn, project_id, reverse);
    let outcome = bfs_core(
        &mut |node, queue| lazy.edges_of(node, queue),
        start,
        target,
        caps,
        allowed_kinds,
        min_confidence,
    );
    lazy.finish()?;
    Ok(outcome)
}

fn bfs_core(
    neighbors: &mut dyn FnMut(&str, &VecDeque<String>) -> Vec<Edge>,
    start: &str,
    target: Option<&str>,
    caps: &BfsCaps,
    allowed_kinds: Option<&[RelationshipKind]>,
    min_confidence: Option<Confidence>,
) -> BfsOutcome {
    let started_at = Instant::now();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    let mut reached: Vec<Reached> = Vec::new();
    let mut predecessor: HashMap<String, (String, Edge)> = HashMap::new();
    let mut depths: HashMap<String, u32> = HashMap::new();
    let mut edges_examined: usize = 0;
    let mut depth_capped = false;
    let mut truncated_reason: Option<TruncationReason> = None;

    visited.insert(start.to_string());
    depths.insert(start.to_string(), 0);
    queue.push_back(start.to_string());
    reached.push(Reached {
        symbol_id: start.to_string(),
        depth: 0,
        via: None,
        predecessor: None,
    });

    if target == Some(start) {
        return BfsOutcome {
            reached,
            path: Some(Vec::new()),
            truncated: false,
            truncated_reason: None,
        };
    }

    'bfs: while let Some(node) = queue.pop_front() {
        let depth = depths[&node];
        let edges = neighbors(&node, &queue);
        if depth >= caps.max_depth {
            if !edges.is_empty() {
                depth_capped = true;
            }
            continue;
        }
        for edge in &edges {
            if started_at.elapsed() > caps.max_duration {
                truncated_reason = Some(TruncationReason::MaxDuration);
                break 'bfs;
            }
            if let Some(kinds) = allowed_kinds {
                if !kinds.iter().any(|k| k.as_str() == edge.kind) {
                    continue;
                }
            }
            if let Some(min) = min_confidence {
                if confidence_rank(&edge.confidence) < confidence_rank(min.as_str()) {
                    continue;
                }
            }

            edges_examined += 1;
            if edges_examined > caps.max_edges {
                truncated_reason = Some(TruncationReason::MaxEdges);
                break 'bfs;
            }
            if visited.contains(&edge.neighbor_symbol_id) {
                continue;
            }
            if reached.len() >= caps.max_nodes {
                truncated_reason = Some(TruncationReason::MaxNodes);
                break 'bfs;
            }

            visited.insert(edge.neighbor_symbol_id.clone());
            let next_depth = depth + 1;
            depths.insert(edge.neighbor_symbol_id.clone(), next_depth);
            predecessor.insert(
                edge.neighbor_symbol_id.clone(),
                (node.clone(), edge.clone()),
            );
            reached.push(Reached {
                symbol_id: edge.neighbor_symbol_id.clone(),
                depth: next_depth,
                via: Some(edge.clone()),
                predecessor: Some(node.clone()),
            });

            if Some(edge.neighbor_symbol_id.as_str()) == target {
                let path = reconstruct_path(&predecessor, start, &edge.neighbor_symbol_id);
                return BfsOutcome {
                    reached,
                    path: Some(path),
                    truncated: false,
                    truncated_reason: None,
                };
            }

            queue.push_back(edge.neighbor_symbol_id.clone());
        }
    }

    if truncated_reason.is_none() && depth_capped {
        truncated_reason = Some(TruncationReason::MaxDepth);
    }

    BfsOutcome {
        reached,
        path: None,
        truncated: truncated_reason.is_some(),
        truncated_reason,
    }
}

/// Walk `predecessor` back from `target` to `start`, collecting edges in
/// traversal order.
fn reconstruct_path(
    predecessor: &HashMap<String, (String, Edge)>,
    start: &str,
    target: &str,
) -> Vec<Edge> {
    let mut path = Vec::new();
    let mut current = target.to_string();
    while current != start {
        let Some((prev, edge)) = predecessor.get(&current) else {
            break;
        };
        path.push(edge.clone());
        current = prev.clone();
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, repo};
    use codekurve_core::{LanguageId, SourceSpan, Symbol, SymbolKind};

    fn edge(neighbor: &str, kind: RelationshipKind, confidence: Confidence) -> Edge {
        Edge {
            neighbor_symbol_id: neighbor.to_string(),
            kind: kind.as_str().to_string(),
            confidence: confidence.as_str().to_string(),
            provenance: "extracted".to_string(),
        }
    }

    fn caps(max_depth: u32) -> BfsCaps {
        BfsCaps {
            max_depth,
            max_nodes: 1000,
            max_edges: 1000,
            max_duration: Duration::from_secs(5),
        }
    }

    /// Spec scenario "Path found within depth": A -> B via 2 `calls` edges,
    /// depth=5 finds it, `truncated:false`.
    #[test]
    fn path_found_within_depth() {
        let mut adjacency: HashMap<String, Vec<Edge>> = HashMap::new();
        adjacency.insert(
            "a".into(),
            vec![edge("mid", RelationshipKind::Calls, Confidence::Exact)],
        );
        adjacency.insert(
            "mid".into(),
            vec![edge("b", RelationshipKind::Calls, Confidence::Exact)],
        );

        let outcome = bfs(&adjacency, "a", Some("b"), &caps(5), None, None);
        let path = outcome.path.expect("path should be found within depth 5");
        assert_eq!(path.len(), 2);
        assert!(!outcome.truncated);
        assert_eq!(outcome.truncated_reason, None);
    }

    /// Spec scenario "Depth limit exceeded": shortest path is 6 hops,
    /// depth=3 -> no path, `truncated:true reason:max_depth`.
    #[test]
    fn depth_limit_exceeded() {
        let mut adjacency: HashMap<String, Vec<Edge>> = HashMap::new();
        let chain = ["a", "n1", "n2", "n3", "n4", "n5", "b"];
        for pair in chain.windows(2) {
            adjacency.insert(
                pair[0].to_string(),
                vec![edge(pair[1], RelationshipKind::Calls, Confidence::Exact)],
            );
        }

        let outcome = bfs(&adjacency, "a", Some("b"), &caps(3), None, None);
        assert!(outcome.path.is_none());
        assert!(outcome.truncated);
        assert_eq!(outcome.truncated_reason, Some(TruncationReason::MaxDepth));
    }

    /// Spec scenario "Impact truncation": reverse graph exceeds max-nodes ->
    /// `truncated:true` + partial results, never silently incomplete.
    #[test]
    fn reverse_impact_truncation() {
        let mut adjacency: HashMap<String, Vec<Edge>> = HashMap::new();
        // Reverse-keyed adjacency: "root" is depended on by dep1/dep2/dep3.
        adjacency.insert(
            "root".into(),
            vec![
                edge("dep1", RelationshipKind::Calls, Confidence::Exact),
                edge("dep2", RelationshipKind::Calls, Confidence::Exact),
                edge("dep3", RelationshipKind::Calls, Confidence::Exact),
            ],
        );

        let tight_caps = BfsCaps {
            max_depth: 10,
            max_nodes: 2,
            max_edges: 100,
            max_duration: Duration::from_secs(5),
        };
        let outcome = bfs(&adjacency, "root", None, &tight_caps, None, None);
        assert!(outcome.truncated);
        assert_eq!(outcome.truncated_reason, Some(TruncationReason::MaxNodes));
        assert!(!outcome.reached.is_empty());
        assert!(outcome.reached.len() <= 2);
    }

    /// `min_confidence` filters which edges BFS may follow at all: a Low
    /// edge is skipped, so a path only reachable through it is never found.
    #[test]
    fn min_confidence_filters_edges() {
        let mut adjacency: HashMap<String, Vec<Edge>> = HashMap::new();
        adjacency.insert(
            "a".into(),
            vec![edge("b", RelationshipKind::Calls, Confidence::Low)],
        );

        let outcome = bfs(
            &adjacency,
            "a",
            Some("b"),
            &caps(5),
            None,
            Some(Confidence::High),
        );
        assert!(outcome.path.is_none());
        // No cap fired — the target is just unreachable under the filter,
        // not truncated.
        assert!(!outcome.truncated);
    }

    fn symbol(name: &str, kind: SymbolKind) -> Symbol {
        Symbol {
            name: name.to_string(),
            qualified_name: format!("src/graph.ts::{name}"),
            kind,
            language: LanguageId::TypeScript,
            span: SourceSpan {
                start_byte: 0,
                end_byte: 1,
                start_line: 1,
                start_column: 0,
                end_line: 1,
                end_column: 1,
            },
            parent: None,
            signature_fingerprint: String::new(),
            visibility: codekurve_core::Visibility::Default,
            is_partial: false,
            is_record: false,
            partial_ordinal: None,
            roles: Vec::new(),
        }
    }

    /// `load_adjacency` round-trips real `relationships` rows into the
    /// forward/reverse-keyed shape `bfs` expects.
    #[test]
    fn load_adjacency_builds_forward_and_reverse_maps() {
        let mut conn = db::open_in_memory().unwrap();
        let project = repo::upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let files = vec![repo::FileInput {
            relative_path: "src/graph.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 10,
            symbols: vec![
                symbol("A", SymbolKind::Function),
                symbol("B", SymbolKind::Function),
            ],
        }];
        repo::reindex(&mut conn, &project, &files, &[], &[]).unwrap();

        let a_id: String = conn
            .query_row("SELECT id FROM symbols WHERE name = 'A'", [], |r| r.get(0))
            .unwrap();
        let b_id: String = conn
            .query_row("SELECT id FROM symbols WHERE name = 'B'", [], |r| r.get(0))
            .unwrap();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();

        let relationships = vec![repo::RelationshipInput {
            source_symbol_id: a_id.clone(),
            target_symbol_id: Some(b_id.clone()),
            target_external: None,
            kind: RelationshipKind::Calls,
            provenance: codekurve_core::Provenance::Extracted,
            confidence: Confidence::Exact,
            source_file_id: file_id,
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        }];
        repo::reindex(&mut conn, &project, &files, &relationships, &[]).unwrap();

        let forward = load_adjacency(&conn, &project, false).unwrap();
        assert_eq!(forward[&a_id][0].neighbor_symbol_id, b_id);

        let reverse = load_adjacency(&conn, &project, true).unwrap();
        assert_eq!(reverse[&b_id][0].neighbor_symbol_id, a_id);
    }

    /// QRY-2: a graph with a cycle, a diamond and a disconnected component,
    /// returned as (conn, project, ids by name).
    fn seeded_graph() -> (Connection, String, HashMap<String, String>) {
        let mut conn = db::open_in_memory().unwrap();
        let project = repo::upsert_project(&conn, "demo", "/tmp/demo", "hash").unwrap();
        let names = ["A", "B", "C", "D", "E", "X", "Y"];
        let files = vec![repo::FileInput {
            relative_path: "src/graph.ts".to_string(),
            language: "typescript".to_string(),
            content_hash: "test-hash".to_string(),
            modified_ns: 0,
            size_bytes: 10,
            symbols: names
                .iter()
                .map(|n| symbol(n, SymbolKind::Function))
                .collect(),
        }];
        repo::reindex(&mut conn, &project, &files, &[], &[]).unwrap();
        let id = |n: &str| -> String {
            conn.query_row("SELECT id FROM symbols WHERE name = ?1", [n], |r| r.get(0))
                .unwrap()
        };
        let ids: HashMap<String, String> = names.iter().map(|n| (n.to_string(), id(n))).collect();
        let file_id: String = conn
            .query_row("SELECT id FROM files LIMIT 1", [], |r| r.get(0))
            .unwrap();
        let rel = |from: &str, to: &str, confidence: Confidence| repo::RelationshipInput {
            source_symbol_id: ids[from].clone(),
            target_symbol_id: Some(ids[to].clone()),
            target_external: None,
            kind: RelationshipKind::Calls,
            provenance: codekurve_core::Provenance::Extracted,
            confidence,
            source_file_id: file_id.clone(),
            start_line: Some(1),
            start_column: Some(0),
            reason: None,
        };
        // A -> B -> D, A -> C -> D (diamond), D -> E, E -> A (cycle), X -> Y (apart)
        let relationships = vec![
            rel("A", "B", Confidence::Exact),
            rel("A", "C", Confidence::Low),
            rel("B", "D", Confidence::Exact),
            rel("C", "D", Confidence::High),
            rel("D", "E", Confidence::Exact),
            rel("E", "A", Confidence::Medium),
            rel("X", "Y", Confidence::Exact),
        ];
        repo::reindex(&mut conn, &project, &files, &relationships, &[]).unwrap();
        (conn, project, ids)
    }

    fn assert_same(a: &BfsOutcome, b: &BfsOutcome) {
        assert_eq!(a.reached, b.reached);
        assert_eq!(a.path, b.path);
        assert_eq!(a.truncated, b.truncated);
        assert_eq!(a.truncated_reason, b.truncated_reason);
    }

    /// QRY-2: the lazy, frontier-batched traversal returns exactly what the
    /// in-memory one does over the fully loaded adjacency — same nodes, same
    /// order, same path, same truncation.
    #[test]
    fn bfs_db_matches_the_in_memory_bfs() {
        let (conn, project, ids) = seeded_graph();
        for reverse in [false, true] {
            let adjacency = load_adjacency(&conn, &project, reverse).unwrap();
            for start in ["A", "D", "X", "E"] {
                for (target, depth) in [(None, 10), (Some("E"), 10), (Some("D"), 1), (None, 1)] {
                    let target = target.map(|t| ids[t].as_str());
                    let want = bfs(&adjacency, &ids[start], target, &caps(depth), None, None);
                    let got = bfs_db(
                        &conn,
                        &project,
                        reverse,
                        &ids[start],
                        target,
                        &caps(depth),
                        None,
                        None,
                    )
                    .unwrap();
                    assert_same(&want, &got);
                }
            }
            // confidence floor and node cap behave the same too
            let want = bfs(
                &adjacency,
                &ids["A"],
                None,
                &caps(10),
                None,
                Some(Confidence::High),
            );
            let got = bfs_db(
                &conn,
                &project,
                reverse,
                &ids["A"],
                None,
                &caps(10),
                None,
                Some(Confidence::High),
            )
            .unwrap();
            assert_same(&want, &got);

            let tight = BfsCaps {
                max_nodes: 3,
                ..caps(10)
            };
            let want = bfs(&adjacency, &ids["A"], None, &tight, None, None);
            let got = bfs_db(
                &conn, &project, reverse, &ids["A"], None, &tight, None, None,
            )
            .unwrap();
            assert_same(&want, &got);
        }
    }

    /// QRY-2: only the neighbourhood of what the search touches is read, not
    /// the whole project.
    #[test]
    fn lazy_edges_load_only_what_the_search_touches() {
        let (conn, project, ids) = seeded_graph();
        let mut lazy = LazyEdges::new(&conn, &project, false);
        let edges = lazy.edges_of(&ids["X"], &VecDeque::new());
        assert_eq!(edges.len(), 1);
        assert_eq!(
            lazy.loaded_edge_count(),
            1,
            "A..E's six edges must not be loaded"
        );
        assert!(lazy.finish().is_ok());
    }
}
