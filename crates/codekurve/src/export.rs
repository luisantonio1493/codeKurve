//! `codekurve export <output.html>` — one self-contained HTML file picturing
//! a symbol's **bidirectional** neighbourhood (what it reaches, and what
//! reaches it), so the picture explains the symbol's place in the system
//! rather than only its blast radius.
//!
//! This is a generated artifact, not a web UI: no server, no daemon, no
//! network, nothing running. The file opens over `file://` and renders
//! identically with the network cable unplugged. See
//! `docs/adr/0013-html-subgraph-export.md`.
//!
//! Split follows `query.rs`'s print/return convention one step further:
//! [`build`] does the database work, [`render`] is a pure
//! `(nodes, edges, focus) -> String` function, and [`run`] is the only part
//! that touches the filesystem or stdout.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use codekurve_store::traverse::{self, BfsOutcome};

use crate::commands::{self, CommandError, QueryArgs};
use crate::query::{self, Session};

/// Hops from the focus symbol when `--depth` is not given. Two is enough to
/// show "who calls me / what do I use, and one step past that" while staying
/// legible on two rings.
const DEFAULT_DEPTH: u32 = 2;

/// One node in the exported picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub id: String,
    /// The readable tail of the qualified name — see [`local_name`].
    pub label: String,
    pub qualified_name: String,
    /// `path:line`, shown as secondary text under the label.
    pub location: String,
    /// Hops from the focus symbol, in either direction; drives the ring.
    pub depth: u32,
}

/// One edge, in real relationship direction (`from` is the relationship's
/// source even when it was discovered by the reverse walk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphEdge {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub confidence: String,
    pub provenance: String,
}

/// Everything [`render`] needs. Deliberately owns no `Connection`: the whole
/// point of this shape is that the HTML can be generated — and tested —
/// without a database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<GraphEdge>,
    /// Symbol id of the centre node.
    pub focus: String,
    /// `Some(reason)` when a BFS cap fired: the picture is incomplete and
    /// says so in the HTML itself, not only on stderr.
    pub truncated: Option<String>,
}

// ---------------------------------------------------------------------------
// Command
// ---------------------------------------------------------------------------

/// `codekurve export <output.html> --symbol-name <name>|--symbol-id <id>
/// [--depth N] [--root <path>] [--yes]`.
pub fn run(args: &QueryArgs, output: &Path, yes: bool) -> Result<(), CommandError> {
    // Checked before any work: an export that clobbers a file the user meant
    // to keep is worse than one that refuses.
    if output.exists() && !yes {
        return Err(CommandError::from(format!(
            "refusing to overwrite existing file {}; pass --yes to replace it",
            output.display()
        )));
    }

    let session = Session::open(args.root)?;
    if let Session::Indexed {
        conn, project_id, ..
    } = &session
    {
        for w in query::pending_warning(conn, project_id) {
            eprintln!("warning: {w}");
        }
    }

    let graph = build(&session, args)?;
    std::fs::write(output, render(&graph))
        .map_err(|e| CommandError::from(format!("could not write {}: {e}", output.display())))?;

    println!(
        "wrote {} ({} node(s), {} edge(s))",
        output.display(),
        graph.nodes.len(),
        graph.edges.len()
    );
    if let Some(reason) = &graph.truncated {
        eprintln!("warning: neighbourhood truncated ({reason}); the export says so too");
    }
    Ok(())
}

/// Bidirectional neighbourhood: the existing bounded BFS (`traverse::bfs`)
/// run once forward and once over the reverse-keyed adjacency, then merged.
/// No new traversal — the caps, the truncation reasons and the
/// `min_confidence` filter are exactly the ones `trace`/`impact` already
/// respect.
fn build(session: &Session, args: &QueryArgs) -> Result<Graph, CommandError> {
    let (conn, project_id) = session.indexed()?;
    let focus = commands::resolve_symbol(conn, project_id, args.symbol_id, args.symbol_name)?;
    let min_confidence = commands::parse_confidence(args.min_confidence)?;
    let caps = commands::bfs_caps(Some(args.depth.unwrap_or(DEFAULT_DEPTH)));

    let mut outcomes = Vec::new();
    for reverse in [false, true] {
        let adjacency = traverse::load_adjacency(conn, project_id, reverse)
            .map_err(|e| CommandError::from(e.to_string()))?;
        outcomes.push(traverse::bfs(
            &adjacency,
            &focus,
            None,
            &caps,
            None,
            min_confidence,
        ));
    }
    merge(session, &focus, &outcomes[0], &outcomes[1])
}

/// Fold the forward and reverse walks into one node/edge set.
///
/// Each walk is a BFS *tree*: `Reached::via` records only the edge that first
/// reached a node, so a node reachable two ways contributes one edge per
/// direction, not all of them. That is what keeps the picture readable, and
/// it is the same set `trace`/`impact` already report.
fn merge(
    session: &Session,
    focus: &str,
    forward: &BfsOutcome,
    reverse: &BfsOutcome,
) -> Result<Graph, CommandError> {
    let mut nodes: Vec<Node> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut seen_edges: HashMap<(String, String, String), ()> = HashMap::new();

    for (outcome, is_reverse) in [(forward, false), (reverse, true)] {
        for r in &outcome.reached {
            match seen.get(&r.symbol_id) {
                // A node found by both walks keeps the shorter hop count —
                // the ring should show the closest way to the focus.
                Some(&i) => nodes[i].depth = nodes[i].depth.min(r.depth),
                None => {
                    seen.insert(r.symbol_id.clone(), nodes.len());
                    nodes.push(describe(session, &r.symbol_id, r.depth));
                }
            }
            let (Some(via), Some(pred)) = (&r.via, &r.predecessor) else {
                continue;
            };
            // The reverse walk keys adjacency by target, so a hop from
            // `pred` to `r.symbol_id` is really the edge `r.symbol_id ->
            // pred`. Store every edge in real relationship direction.
            let (from, to) = if is_reverse {
                (r.symbol_id.clone(), pred.clone())
            } else {
                (pred.clone(), r.symbol_id.clone())
            };
            let key = (from.clone(), to.clone(), via.kind.clone());
            if seen_edges.insert(key, ()).is_none() {
                edges.push(GraphEdge {
                    from,
                    to,
                    kind: via.kind.clone(),
                    confidence: via.confidence.clone(),
                    provenance: via.provenance.clone(),
                });
            }
        }
    }

    let truncated = [forward, reverse]
        .iter()
        .find_map(|o| o.truncated_reason)
        .map(|r| truncation_reason(r).to_string());

    Ok(Graph {
        nodes,
        edges,
        focus: focus.to_string(),
        truncated,
    })
}

/// A node's display fields, falling back to the bare id when the row is gone
/// from a stale index — one lookup miss must not fail the whole export.
fn describe(session: &Session, id: &str, depth: u32) -> Node {
    match query::get_symbol(session, id, 0) {
        Ok(d) => Node {
            id: id.to_string(),
            label: local_name(&d.symbol.qualified_name),
            qualified_name: d.symbol.qualified_name.clone(),
            location: format!("{}:{}", d.symbol.relative_path, d.symbol.span.start_line),
            depth,
        },
        Err(_) => Node {
            id: id.to_string(),
            label: id.to_string(),
            qualified_name: id.to_string(),
            location: String::new(),
            depth,
        },
    }
}

/// The readable tail of a qualified name
/// (`Source/Models/TodoItem.cs::MinimalApi.Models.TodoItem` ->
/// `MinimalApi.Models.TodoItem`), the treatment `codekurve-tui`'s explorer
/// settled on: CodeKurve qualified names embed the file path, so printing the
/// whole one next to the `path:line` secondary text repeats the path twice
/// and overflows the label.
fn local_name(qualified: &str) -> String {
    qualified
        .rsplit("::")
        .next()
        .unwrap_or(qualified)
        .to_string()
}

fn truncation_reason(r: traverse::TruncationReason) -> &'static str {
    match r {
        traverse::TruncationReason::MaxDepth => "max_depth",
        traverse::TruncationReason::MaxNodes => "max_nodes",
        traverse::TruncationReason::MaxEdges => "max_edges",
        traverse::TruncationReason::MaxDuration => "max_duration",
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Distance between rings, in SVG units.
const RING: f64 = 210.0;
/// Padding outside the outermost ring, leaving room for labels.
const MARGIN: f64 = 120.0;
/// Minimum arc between two nodes on the same ring. A hub symbol can put
/// hundreds of nodes on one ring; without this they overlap into a solid
/// band. Pushing the ring outward until every node has this much room keeps
/// the layout readable and still a pure function of the node count.
const ARC: f64 = 58.0;

/// `(nodes, edges, focus) -> String`. Pure: same input, byte-identical
/// output, so the artifact is diffable and testable without a database.
///
/// Layout is **radial by BFS depth**, computed here rather than by a
/// force-directed simulation in the browser, for three reasons:
///
/// 1. It needs no JS layout library, which is what keeps the file genuinely
///    self-contained instead of carrying ~100-280 KB of inlined d3/cytoscape.
/// 2. It is deterministic — the same index always produces the same file.
/// 3. Radial distance *means* something here (hops from the focus), whereas
///    the distance a force layout settles on means nothing at all.
pub fn render(g: &Graph) -> String {
    let max_depth = g.nodes.iter().map(|n| n.depth).max().unwrap_or(0);

    // Ring by ring, angles distributed evenly; index within the ring is the
    // node's position in `g.nodes`, so the geometry is a function of the
    // node list alone.
    let rings: Vec<Vec<&Node>> = (0..=max_depth)
        .map(|d| g.nodes.iter().filter(|n| n.depth == d).collect())
        .collect();
    // A ring never sits closer in than the previous one, and never so close
    // that its own nodes would overlap.
    let mut radii: Vec<f64> = Vec::with_capacity(rings.len());
    for (depth, ring) in rings.iter().enumerate() {
        if depth == 0 {
            // The focus symbol is the centre, always.
            radii.push(0.0);
            continue;
        }
        let spaced = ring.len() as f64 * ARC / std::f64::consts::TAU;
        let previous = radii.last().copied().unwrap_or(0.0);
        radii.push((depth as f64 * RING).max(spaced).max(previous + RING));
    }
    let size = 2.0 * (radii.last().copied().unwrap_or(0.0) + MARGIN);
    let centre = size / 2.0;

    let mut pos: HashMap<&str, (f64, f64)> = HashMap::new();
    for (depth, ring) in rings.iter().enumerate() {
        let count = ring.len() as f64;
        let radius = radii[depth];
        for (i, node) in ring.iter().enumerate() {
            let angle = -std::f64::consts::FRAC_PI_2 + std::f64::consts::TAU * (i as f64) / count;
            pos.insert(
                node.id.as_str(),
                (centre + radius * angle.cos(), centre + radius * angle.sin()),
            );
        }
    }

    let focus_label = g
        .nodes
        .iter()
        .find(|n| n.id == g.focus)
        .map_or_else(|| g.focus.clone(), |n| n.qualified_name.clone());

    let mut out = String::with_capacity(4096);
    out.push_str(HEAD_OPEN);
    let _ = write!(out, "codekurve — {}", esc(&local_name(&focus_label)));
    out.push_str(HEAD_CLOSE);

    let _ = writeln!(
        out,
        "<h1>{}</h1>\n<p class=\"sub\">{} node(s), {} edge(s) · {} hop(s) either way</p>",
        esc(&focus_label),
        g.nodes.len(),
        g.edges.len(),
        max_depth
    );
    if let Some(reason) = &g.truncated {
        // `max_depth` is the requested boundary doing its job; the other
        // three are caps firing early. Both leave nodes out, but the honest
        // advice differs, and saying "re-run with a smaller --depth" to
        // someone who hit `max_depth` would be nonsense.
        let advice = if reason == "max_depth" {
            "the neighbourhood stops at the requested --depth, so symbols further out are not \
             shown. Raise --depth to see more."
        } else {
            "a traversal cap fired before the neighbourhood was exhausted, so nodes and edges \
             are missing. Lower --depth or add --min-confidence for a complete view of a \
             narrower neighbourhood."
        };
        let _ = writeln!(
            out,
            "<p class=\"warn\">Truncated ({}): {advice}</p>",
            esc(reason)
        );
    }

    let _ = writeln!(
        out,
        // No `xmlns`: inline SVG inside an HTML document is already in the
        // SVG namespace, and the attribute's value is the one string in this
        // file that would look like an external URL to a reader (or a grep)
        // checking the self-contained rule.
        "<svg id=\"g\" viewBox=\"0 0 {size:.0} {size:.0}\" role=\"img\" \
         aria-label=\"code graph neighbourhood\">"
    );

    // Rings first, then edges, then nodes — painter's order.
    for radius in radii.iter().skip(1) {
        let _ = writeln!(
            out,
            "<circle class=\"ring\" cx=\"{centre:.1}\" cy=\"{centre:.1}\" r=\"{radius:.1}\"/>"
        );
    }

    for e in &g.edges {
        let (Some(&(x1, y1)), Some(&(x2, y2))) = (pos.get(e.from.as_str()), pos.get(e.to.as_str()))
        else {
            continue;
        };
        let dash = if e.provenance == "heuristic" {
            " stroke-dasharray=\"7 5\""
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "<line class=\"edge\" data-from=\"{}\" data-to=\"{}\" x1=\"{x1:.1}\" y1=\"{y1:.1}\" \
             x2=\"{x2:.1}\" y2=\"{y2:.1}\" stroke=\"{}\" stroke-opacity=\"{:.2}\"{dash}>\
             <title>{} → {} · {} · {} · {}</title></line>",
            esc(&e.from),
            esc(&e.to),
            kind_colour(&e.kind),
            confidence_opacity(&e.confidence),
            esc(&label_of(g, &e.from)),
            esc(&label_of(g, &e.to)),
            esc(&e.kind),
            esc(&e.confidence),
            esc(&e.provenance),
        );
    }

    for n in &g.nodes {
        let Some(&(x, y)) = pos.get(n.id.as_str()) else {
            continue;
        };
        let focus = if n.id == g.focus { " focus" } else { "" };
        let r = if n.id == g.focus { 13.0 } else { 8.0 };
        let _ = writeln!(
            out,
            "<g class=\"node{focus}\" data-id=\"{}\" tabindex=\"0\">\
             <title>{}\n{}</title>\
             <circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{r:.1}\"/>\
             <text x=\"{x:.1}\" y=\"{:.1}\">{}</text>\
             <text class=\"loc\" x=\"{x:.1}\" y=\"{:.1}\">{}</text></g>",
            esc(&n.id),
            esc(&n.qualified_name),
            esc(&n.location),
            y - r - 16.0,
            esc(&n.label),
            y - r - 4.0,
            esc(&n.location),
        );
    }
    out.push_str("</svg>\n");

    out.push_str("<div class=\"legend\"><h2>Edge kind</h2><ul>\n");
    let mut kinds: Vec<&str> = g.edges.iter().map(|e| e.kind.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    for kind in kinds {
        let _ = writeln!(
            out,
            "<li><span class=\"swatch\" style=\"background:{}\"></span>{}</li>",
            kind_colour(kind),
            esc(kind)
        );
    }
    out.push_str(LEGEND_PROVENANCE);
    out.push_str(FOOT);
    out
}

/// A node's label for edge tooltips, by id.
fn label_of(g: &Graph, id: &str) -> String {
    g.nodes
        .iter()
        .find(|n| n.id == id)
        .map_or_else(|| id.to_string(), |n| n.label.clone())
}

/// One colour per relationship kind. Framework-inferred kinds (`injects`,
/// `registeredas`, `handlesroute`, `triggers`, `persiststo`) share a warm
/// family so they read as one group; the dashed stroke, not the colour, is
/// what marks them as inferences (see [`render`] and ADR 0007).
fn kind_colour(kind: &str) -> &'static str {
    match kind {
        "calls" => "#3b82f6",
        "constructs" => "#6366f1",
        "inherits" | "implements" | "overrides" => "#14b8a6",
        "imports" | "exports" => "#64748b",
        "defines" | "contains" => "#94a3b8",
        "reads" | "writes" => "#a855f7",
        "usestype" | "references" => "#0ea5e9",
        "decorates" => "#eab308",
        "injects" => "#f97316",
        "registeredas" => "#ef4444",
        "handlesroute" => "#ec4899",
        "triggers" => "#f43f5e",
        "persiststo" => "#d946ef",
        _ => "#9ca3af",
    }
}

/// Confidence as stroke opacity. Deliberately never reaches full opacity for
/// anything below `exact`, and never drops to invisible: the encoding must
/// not imply more certainty than the stored value carries, nor hide an edge
/// the graph does contain. The exact value is also in the edge's tooltip.
fn confidence_opacity(confidence: &str) -> f64 {
    match confidence {
        "exact" => 1.0,
        "high" => 0.8,
        "medium" => 0.6,
        "low" => 0.4,
        _ => 0.25,
    }
}

/// Minimal HTML escaping for text and attribute values.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

// Everything below is inlined on purpose: no CDN, no external font, no remote
// image, no fetch. The file must render identically with no network at all.
const HEAD_OPEN: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>";

const HEAD_CLOSE: &str = "</title>\n<style>\n\
:root { color-scheme: light dark; --fg: #111; --muted: #667; --bg: #fff; --line: #dde; }\n\
@media (prefers-color-scheme: dark) { :root { --fg: #e8e8ea; --muted: #9aa; --bg: #16161a; --line: #333; } }\n\
body { margin: 0; padding: 24px; background: var(--bg); color: var(--fg);\n\
  font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif; }\n\
h1 { font-size: 18px; margin: 0 0 4px; word-break: break-all; }\n\
h2 { font-size: 12px; text-transform: uppercase; letter-spacing: .06em; color: var(--muted); margin: 12px 0 6px; }\n\
.sub { color: var(--muted); margin: 0 0 12px; }\n\
.warn { border-left: 3px solid #f59e0b; background: #f59e0b22; padding: 8px 12px; margin: 0 0 12px; }\n\
svg { width: 100%; height: auto; max-height: 78vh; display: block; }\n\
.ring { fill: none; stroke: var(--line); stroke-dasharray: 2 6; }\n\
.edge { stroke-width: 1.6; }\n\
.node circle { fill: var(--bg); stroke: var(--fg); stroke-width: 2; }\n\
.node.focus circle { fill: #3b82f6; stroke: #3b82f6; }\n\
.node text { text-anchor: middle; font-size: 13px; fill: var(--fg); }\n\
.node text.loc { font-size: 10px; fill: var(--muted); }\n\
.node { cursor: pointer; }\n\
.dim { opacity: .07; }\n\
.legend { border-top: 1px solid var(--line); padding-top: 8px; }\n\
.legend ul { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: 4px 18px; }\n\
.legend li { display: flex; align-items: center; gap: 6px; color: var(--muted); }\n\
.swatch { width: 22px; height: 3px; border-radius: 2px; display: inline-block; }\n\
.swatch.dash { background: none; border-top: 3px dashed var(--fg); }\n\
.swatch.solid { background: var(--fg); }\n\
</style>\n</head>\n<body>\n";

/// The provenance half of the legend. Constant, and always shown: a viewer
/// must be able to tell at a glance which edges are framework inferences
/// (`docs/FRAMEWORKS.md`, ADR 0007) — an inferred `registeredas` must never
/// look like a parsed fact.
const LEGEND_PROVENANCE: &str = "</ul>\n<h2>Provenance and confidence</h2><ul>\n\
<li><span class=\"swatch solid\"></span>solid — extracted or resolved: parsed from the source</li>\n\
<li><span class=\"swatch dash\"></span>dashed — heuristic: inferred by framework recognition, not a parsed fact</li>\n\
<li>opacity — confidence (exact, high, medium, low); hover an edge for the exact value</li>\n\
</ul></div>\n";

// Hand-written, dependency-free: click (or keyboard-focus) a node to keep
// only its incident edges lit. No pan/zoom library, no package.
const FOOT: &str = "<script>\n\
(function () {\n\
  var svg = document.getElementById('g');\n\
  var edges = svg.querySelectorAll('.edge');\n\
  var active = null;\n\
  function apply(id) {\n\
    active = id;\n\
    for (var i = 0; i < edges.length; i++) {\n\
      var e = edges[i];\n\
      var on = !id || e.dataset.from === id || e.dataset.to === id;\n\
      e.classList.toggle('dim', !on);\n\
    }\n\
  }\n\
  svg.addEventListener('click', function (ev) {\n\
    var g = ev.target.closest('.node');\n\
    apply(g && g.dataset.id !== active ? g.dataset.id : null);\n\
  });\n\
})();\n\
</script>\n</body>\n</html>\n";

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, label: &str, depth: u32) -> Node {
        Node {
            id: id.to_string(),
            label: label.to_string(),
            qualified_name: format!("src/a.ts::{label}"),
            location: "src/a.ts:1".to_string(),
            depth,
        }
    }

    fn edge(from: &str, to: &str, provenance: &str) -> GraphEdge {
        GraphEdge {
            from: from.to_string(),
            to: to.to_string(),
            kind: "calls".to_string(),
            confidence: "exact".to_string(),
            provenance: provenance.to_string(),
        }
    }

    fn sample() -> Graph {
        Graph {
            nodes: vec![
                node("a", "Focus", 0),
                node("b", "Callee", 1),
                node("c", "Caller", 1),
            ],
            edges: vec![edge("a", "b", "extracted"), edge("c", "a", "resolved")],
            focus: "a".to_string(),
            truncated: None,
        }
    }

    /// A known small graph renders the same bytes every time — the whole
    /// point of computing the layout in Rust instead of in a simulation.
    #[test]
    fn render_is_deterministic() {
        let g = sample();
        assert_eq!(render(&g), render(&g));
        assert_eq!(render(&g), render(&sample()));
    }

    /// The focus sits at the exact centre, and a crowded ring pushes itself
    /// outward instead of overlapping into a solid band.
    #[test]
    fn focus_is_centred_and_crowded_rings_expand() {
        let mut g = sample();
        assert!(render(&g).contains("<circle cx=\"330.0\" cy=\"330.0\" r=\"13.0\"/>"));

        for i in 0..200 {
            g.nodes.push(node(&format!("n{i}"), &format!("N{i}"), 1));
        }
        let html = render(&g);
        // 202 nodes on ring 1 need far more than the nominal 210 units of
        // radius; the ring circle proves it grew.
        let r: f64 = html
            .split("class=\"ring\"")
            .nth(1)
            .and_then(|s| s.split("r=\"").nth(1))
            .and_then(|s| s.split('"').next())
            .unwrap()
            .parse()
            .unwrap();
        assert!(r > 1800.0, "crowded ring did not expand: {r}");
    }

    /// A heuristic edge and an extracted edge must not look the same
    /// (ADR 0007): dashed versus solid, plus the legend that says so.
    #[test]
    fn heuristic_edges_render_differently_from_extracted_ones() {
        let mut g = sample();
        g.edges.push(edge("b", "c", "heuristic"));
        let html = render(&g);

        assert_eq!(html.matches("stroke-dasharray=\"7 5\"").count(), 1);
        assert!(html.contains("dashed — heuristic"));
        assert!(html.contains("solid — extracted or resolved"));

        // ...and without a heuristic edge, no edge is dashed.
        assert!(!render(&sample()).contains("stroke-dasharray=\"7 5\""));
    }

    /// Truncation is admitted in the HTML itself, not only on stderr.
    #[test]
    fn truncation_notice_appears_only_when_truncated() {
        assert!(!render(&sample()).contains("Truncated"));

        let mut g = sample();
        g.truncated = Some("max_nodes".to_string());
        let html = render(&g);
        assert!(html.contains("Truncated (max_nodes)"));
    }

    /// Labels use the tail of the qualified name; the duplicated-path form
    /// (`src/a.ts::Callee` next to `src/a.ts:1`) never reaches a label.
    #[test]
    fn node_labels_do_not_duplicate_the_path() {
        let html = render(&sample());
        assert!(html.contains(">Callee</text>"));
        assert!(!html.contains(">src/a.ts::Callee</text>"));
        assert_eq!(local_name("src/db.ts::TodoDbContext"), "TodoDbContext");
        assert_eq!(local_name("bare"), "bare");
    }

    /// Self-contained: not one URL of any kind in the output.
    #[test]
    fn output_references_no_external_asset() {
        let html = render(&sample());
        assert!(!html.contains("http://"));
        assert!(!html.contains("https://"));
        assert!(!html.contains("//cdn"));
    }
}
