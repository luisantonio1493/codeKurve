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
         WHERE project_id = ?1 AND target_symbol_id IS NOT NULL",
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
        if depth >= caps.max_depth {
            if adjacency.get(&node).is_some_and(|e| !e.is_empty()) {
                depth_capped = true;
            }
            continue;
        }
        let Some(edges) = adjacency.get(&node) else {
            continue;
        };
        for edge in edges {
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
}
