//! `codekurve tui` — the code-graph explorer.
//!
//! Layout: a search box on top, the symbol hits on the left, the selected
//! symbol's detail on the right, one hint line at the bottom.
//!
//! ```text
//! ┌─ codekurve ────────────────────────────────────────┐
//! │ search: TodoItem█                                  │
//! ├─ symbols ─────────┬─ TodoItem (class) ─────────────┤
//! │ > TodoItem  class │ Source/Models/TodoItem.cs:5    │
//! │   TodoItemInput   │                                │
//! │   TodoItemOutput  │ references (4)                 │
//! │                   │  ← TodoDbContext   persiststo  │
//! │                   │  ← CreateTodoItem  constructs  │
//! └───────────────────┴────────────────────────────────┘
//! ```
//!
//! The right panel lists **references**, not callers: a framework-invoked
//! symbol (a controller action, a DI-registered service, an Angular
//! component) has no `Calls` edge pointing at it, so `find_callers` would
//! render an empty panel for exactly the symbols a user most wants to walk
//! from (`docs/AGENT_USAGE.md`, "`find_callers` empty ≠ nothing calls
//! this"). `RelKind::References` returns every edge kind, framework edges
//! included.
//!
//! [`Explorer`] holds the whole state machine and contains no `ratatui`,
//! `crossterm` or database types — every panel's content arrives through a
//! setter, so selection, navigation and search transitions are unit-testable
//! without a terminal.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use codekurve::commands::{CommandError, QueryArgs};
use codekurve::query::{self, RelKind, SearchInput, Session};

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::{read_key, step, Key};

/// Relationship rows fetched per symbol. Bounded for the same reason every
/// other CodeKurve query is (§27.5): a panel is not a place to stream 50k
/// edges, and the count line still reports the untruncated total.
const REL_LIMIT: usize = 500;

/// One row in the left-hand results list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolRow {
    pub id: String,
    pub name: String,
    pub kind: String,
}

/// One relationship row in the right-hand panel. `target` is `None` for an
/// edge pointing at an external (unindexed) symbol — there is nothing to
/// navigate to, so `↵` reports that instead of jumping nowhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelRow {
    pub target: Option<String>,
    pub name: String,
    pub kind: String,
}

/// One reached node in the impact view. `label` is always a resolved
/// `name (path:line)` — never a bare `sym-*` id (see [`describe`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactRow {
    pub label: String,
    pub depth: u32,
}

/// Everything loaded for the symbol currently shown on the right.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Detail {
    pub title: String,
    pub location: String,
    pub rels: Vec<RelRow>,
    /// Total before [`REL_LIMIT`] truncation.
    pub total_rels: usize,
    /// Loaded lazily, only once `i` is pressed — `query::impact` loads the
    /// project's whole adjacency list per call, far too heavy to run on
    /// every arrow key.
    pub impact: Option<Vec<ImpactRow>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Search,
    Results,
    Rels,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Rels,
    Impact,
}

pub struct Explorer {
    pub query: String,
    pub results: Vec<SymbolRow>,
    pub sel: usize,
    pub focus: Focus,
    /// Symbol id the right panel describes. Follows the results selection
    /// until a relationship row is entered, after which it walks the graph
    /// independently of the list.
    pub current: Option<String>,
    pub detail: Option<Detail>,
    pub rel_sel: usize,
    /// Symbols visited before `current`, most recent last — `Esc`/
    /// `Backspace` pops one.
    pub back: Vec<String>,
    pub pane: Pane,
    pub warnings: Vec<String>,
    pub message: Option<String>,
    /// `query` changed since the last [`set_results`]; the event loop
    /// re-runs the search before the next draw.
    pub search_dirty: bool,
    pub quit: bool,
}

impl Explorer {
    pub fn new(warnings: Vec<String>) -> Self {
        Self {
            query: String::new(),
            results: Vec::new(),
            sel: 0,
            focus: Focus::Search,
            current: None,
            detail: None,
            rel_sel: 0,
            back: Vec::new(),
            pane: Pane::Rels,
            warnings,
            message: None,
            search_dirty: false,
            quit: false,
        }
    }

    /// The symbol whose detail still has to be loaded, if any.
    pub fn pending_detail(&self) -> Option<&str> {
        match (&self.current, &self.detail) {
            (Some(id), None) => Some(id.as_str()),
            _ => None,
        }
    }

    /// The symbol whose impact still has to be computed, if any — only ever
    /// `Some` while the impact pane is open.
    pub fn pending_impact(&self) -> Option<&str> {
        if self.pane != Pane::Impact {
            return None;
        }
        match (&self.current, &self.detail) {
            (Some(id), Some(d)) if d.impact.is_none() => Some(id.as_str()),
            _ => None,
        }
    }

    /// A fresh result set resets the walk: selection to the top, the right
    /// panel to the first hit, and the back-stack cleared — its entries
    /// belong to the previous query and `Esc` jumping into them would land
    /// on a symbol the user can no longer see.
    pub fn set_results(&mut self, rows: Vec<SymbolRow>) {
        self.current = rows.first().map(|r| r.id.clone());
        self.results = rows;
        self.sel = 0;
        self.back.clear();
        self.reset_detail();
        self.search_dirty = false;
        if self.focus == Focus::Rels {
            self.focus = Focus::Results;
        }
    }

    pub fn set_detail(&mut self, detail: Detail) {
        self.detail = Some(detail);
        self.rel_sel = 0;
    }

    pub fn set_impact(&mut self, rows: Vec<ImpactRow>) {
        if let Some(d) = self.detail.as_mut() {
            d.impact = Some(rows);
        }
    }

    fn reset_detail(&mut self) {
        self.detail = None;
        self.rel_sel = 0;
        self.pane = Pane::Rels;
    }

    /// Make `id` the symbol on the right, remembering where we came from.
    pub fn navigate_to(&mut self, id: String) {
        if let Some(prev) = self.current.replace(id) {
            self.back.push(prev);
        }
        self.reset_detail();
        self.focus = Focus::Rels;
    }

    /// Undo one [`navigate_to`]. `false` when there is nothing to undo, so
    /// the caller can give the key a second meaning.
    pub fn go_back(&mut self) -> bool {
        let Some(prev) = self.back.pop() else {
            return false;
        };
        self.current = Some(prev);
        self.reset_detail();
        true
    }

    fn move_result(&mut self, down: bool) {
        let next = step(self.results.len(), self.sel, down);
        if next == self.sel && self.current.is_some() {
            return;
        }
        self.sel = next;
        self.current = self.results.get(next).map(|r| r.id.clone());
        self.back.clear();
        self.reset_detail();
    }

    fn move_rel(&mut self, down: bool) {
        self.rel_sel = step(self.rel_len(), self.rel_sel, down);
    }

    fn rel_len(&self) -> usize {
        match (&self.detail, self.pane) {
            (Some(d), Pane::Rels) => d.rels.len(),
            (Some(d), Pane::Impact) => d.impact.as_ref().map_or(0, Vec::len),
            (None, _) => 0,
        }
    }

    /// Move focus to the relationship list, if there is one to focus.
    fn focus_rels(&mut self) {
        if self.rel_len() > 0 {
            self.focus = Focus::Rels;
        } else {
            self.focus = Focus::Results;
        }
    }

    fn enter_rel(&mut self) {
        if self.pane == Pane::Impact {
            self.message = Some("impact rows are read-only; press i to go back".into());
            return;
        }
        let Some(row) = self
            .detail
            .as_ref()
            .and_then(|d| d.rels.get(self.rel_sel))
            .cloned()
        else {
            return;
        };
        match row.target {
            Some(id) => self.navigate_to(id),
            None => self.message = Some(format!("{} is external; not in this index", row.name)),
        }
    }

    fn toggle_impact(&mut self) {
        self.pane = match self.pane {
            Pane::Rels => Pane::Impact,
            Pane::Impact => Pane::Rels,
        };
        self.rel_sel = 0;
    }

    pub fn on_key(&mut self, key: Key) {
        self.message = None;
        if key == Key::Quit {
            self.quit = true;
            return;
        }
        match self.focus {
            Focus::Search => self.on_key_search(key),
            _ => self.on_key_list(key),
        }
    }

    /// While the search box has focus every printable character is text, so
    /// `q` cannot mean "quit" here — `Esc` (or `↵`) leaves the box first,
    /// and `Ctrl-C` always works.
    fn on_key_search(&mut self, key: Key) {
        match key {
            Key::Char(c) => {
                self.query.push(c);
                self.search_dirty = true;
            }
            Key::Backspace => {
                self.query.pop();
                self.search_dirty = true;
            }
            Key::Up => self.move_result(false),
            Key::Down => self.move_result(true),
            Key::Enter | Key::Tab | Key::Right => self.focus_rels(),
            Key::Esc => self.focus = Focus::Results,
            _ => {}
        }
    }

    fn on_key_list(&mut self, key: Key) {
        match (self.focus, key) {
            (_, Key::Char('q')) => self.quit = true,
            (_, Key::Char('/')) => self.focus = Focus::Search,
            (_, Key::Char('i')) => self.toggle_impact(),
            (Focus::Rels, Key::Up) => self.move_rel(false),
            (Focus::Rels, Key::Down) => self.move_rel(true),
            (Focus::Rels, Key::Enter) => self.enter_rel(),
            (Focus::Rels, Key::Left | Key::Tab) => self.focus = Focus::Results,
            (Focus::Results, Key::Up) => self.move_result(false),
            (Focus::Results, Key::Down) => self.move_result(true),
            (Focus::Results, Key::Enter | Key::Tab | Key::Right) => self.focus_rels(),
            (_, Key::Esc | Key::Backspace) => self.back_or_search(),
            _ => {}
        }
    }

    /// `Esc`/`Backspace` walk the navigation stack back; once it is empty
    /// the same key drops into the search box rather than doing nothing.
    fn back_or_search(&mut self) {
        if !self.go_back() {
            self.focus = Focus::Search;
        }
    }
}

// ---------------------------------------------------------------------------
// Data loading — every call goes through `codekurve::query`.
// ---------------------------------------------------------------------------

fn args<'a>(root: &'a Path, id: &'a str) -> QueryArgs<'a> {
    QueryArgs {
        root,
        symbol_id: Some(id),
        symbol_name: None,
        min_confidence: None,
        depth: None,
        limit: Some(REL_LIMIT),
        offset: None,
        json: false,
    }
}

fn load_results(session: &Session, q: &str) -> Result<Vec<SymbolRow>, CommandError> {
    if q.trim().is_empty() {
        return Ok(Vec::new());
    }
    let page = query::search(
        session,
        &SearchInput {
            query: q,
            limit: None,
        },
    )?;
    Ok(page
        .rows
        .into_iter()
        .map(|s| SymbolRow {
            id: s.id,
            name: s.name,
            kind: s.kind,
        })
        .collect())
}

fn load_detail(session: &Session, root: &Path, id: &str) -> Result<Detail, CommandError> {
    let sym = query::get_symbol(session, id, 0)?.symbol;
    let page = query::relationships(session, RelKind::References, &args(root, id))?;
    Ok(Detail {
        title: format!("{} ({})", sym.name, sym.kind),
        location: format!("{}:{}", sym.relative_path, sym.span.start_line),
        rels: page
            .rows
            .iter()
            .map(|r| RelRow {
                target: Some(r.source_symbol_id.clone()),
                name: local_name(&r.source_qualified_name),
                kind: r.kind.clone(),
            })
            .collect(),
        total_rels: page.total,
        impact: None,
    })
}

fn load_impact(session: &Session, root: &Path, id: &str) -> Result<Vec<ImpactRow>, CommandError> {
    let outcome = query::impact(session, &args(root, id))?;
    Ok(outcome
        .reached
        .iter()
        .map(|r| ImpactRow {
            label: describe(session, &r.symbol_id),
            depth: r.depth,
        })
        .collect())
}

/// `commands::describe_symbol`'s resolution, reproduced through the public
/// query layer (that helper is private to `commands`): a real name plus
/// `path:line`, falling back to the raw id only when the row itself is gone
/// from a stale index — an impact list must never show bare `sym-*` ids.
///
/// Only the display differs from the CLI's: CodeKurve qualified names embed
/// the file (`Source/Models/TodoItem.cs::MinimalApi.Models.TodoItem`), so
/// printing the full one next to `path:line` repeats the path twice and
/// overflows a half-width panel. The tail plus the location carries the same
/// information in the space available.
fn describe(session: &Session, id: &str) -> String {
    match query::get_symbol(session, id, 0) {
        Ok(d) => format!(
            "{} ({}:{})",
            local_name(&d.symbol.qualified_name),
            d.symbol.relative_path,
            d.symbol.span.start_line
        ),
        Err(_) => id.to_string(),
    }
}

/// The readable tail of a qualified name (`src/db.ts::TodoDbContext` ->
/// `TodoDbContext`); a panel this narrow cannot show the path prefix, and
/// the location line above it already carries the file.
fn local_name(qualified: &str) -> String {
    qualified
        .rsplit("::")
        .next()
        .unwrap_or(qualified)
        .to_string()
}

// ---------------------------------------------------------------------------
// Terminal shell
// ---------------------------------------------------------------------------

/// `codekurve tui [--root <path>]`.
///
/// A project with no completed index fails with the CLI's own message
/// ("run `codekurve init`" / "run `codekurve index`") and the CLI's own exit
/// code 4 (§27, "query before first index") rather than opening an empty
/// screen the user cannot act on.
pub fn run(root: &Path) -> Result<(), CommandError> {
    // Checked before `Session::open` so a piped invocation gets this instead
    // of `ratatui::try_init`'s raw OS error ("Device not configured (os error
    // 6)"), which says nothing about what the caller did wrong. Agents and
    // scripts reach for `codekurve tui` by mistake — the CLI subcommands are
    // what they actually want, so name them.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(CommandError::from(
            "codekurve tui needs an interactive terminal (stdin and stdout must both be a TTY).\n\
             For scripted or agent use, the query subcommands print the same data: \
             search, symbol, references, callers, callees, trace, impact (add --json)."
                .to_string(),
        ));
    }

    let session = Session::open(root)?;
    if let Session::NotIndexed { reason, .. } = &session {
        return Err(CommandError {
            code: 4,
            message: reason.clone(),
        });
    }
    let root: PathBuf = session.root().to_path_buf();
    let mut app = Explorer::new(session.warnings());

    let mut terminal = ratatui::try_init().map_err(|e| CommandError::from(e.to_string()))?;
    let outcome = event_loop(&mut terminal, &session, &root, &mut app);
    ratatui::restore();
    outcome.map_err(CommandError::from)
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    session: &Session,
    root: &Path,
    app: &mut Explorer,
) -> Result<(), String> {
    loop {
        refresh(session, root, app);
        terminal
            .draw(|frame| render(frame, app))
            .map_err(|e| e.to_string())?;
        if let Some(key) = read_key()? {
            app.on_key(key);
        }
        if app.quit {
            return Ok(());
        }
    }
}

/// Satisfies whatever the last keypress asked for, before drawing. A failed
/// load becomes a message plus an empty panel — never a retry loop, since
/// the placeholder clears the pending flag.
fn refresh(session: &Session, root: &Path, app: &mut Explorer) {
    if app.search_dirty {
        match load_results(session, &app.query) {
            Ok(rows) => app.set_results(rows),
            Err(e) => {
                app.set_results(Vec::new());
                app.message = Some(e.message);
            }
        }
    }
    if let Some(id) = app.pending_detail().map(str::to_string) {
        match load_detail(session, root, &id) {
            Ok(detail) => app.set_detail(detail),
            Err(e) => {
                app.set_detail(Detail::default());
                app.message = Some(e.message);
            }
        }
    }
    if let Some(id) = app.pending_impact().map(str::to_string) {
        match load_impact(session, root, &id) {
            Ok(rows) => app.set_impact(rows),
            Err(e) => {
                app.set_impact(Vec::new());
                app.message = Some(e.message);
            }
        }
    }
}

const ACCENT: Color = Color::Cyan;

fn render(frame: &mut Frame, app: &Explorer) {
    let warn_rows = u16::from(!app.warnings.is_empty());
    let [search, warn, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(warn_rows),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    frame.render_widget(
        Paragraph::new(format!("search: {}{}", app.query, cursor(app)))
            .block(Block::bordered().title(" codekurve ")),
        search,
    );
    if warn_rows == 1 {
        frame.render_widget(
            Paragraph::new(format!("⚠ {}", app.warnings.join("; ")))
                .style(Style::new().fg(Color::Yellow)),
            warn,
        );
    }

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(35), Constraint::Min(20)]).areas(body);
    render_results(frame, app, left);
    render_detail(frame, app, right);

    frame.render_widget(
        Paragraph::new(footer_hint(app)).style(Style::new().fg(Color::DarkGray)),
        footer,
    );
}

fn cursor(app: &Explorer) -> &'static str {
    if app.focus == Focus::Search {
        "█"
    } else {
        ""
    }
}

fn render_results(frame: &mut Frame, app: &Explorer, area: Rect) {
    let items: Vec<ListItem> = app
        .results
        .iter()
        .map(|r| ListItem::new(format!("{}  {}", r.name, r.kind)))
        .collect();
    let title = format!(" symbols ({}) ", app.results.len());
    let list = List::new(items)
        .block(bordered(&title, app.focus == Focus::Results))
        .highlight_symbol("> ")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(app.sel));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_detail(frame: &mut Frame, app: &Explorer, area: Rect) {
    let Some(detail) = app.detail.as_ref() else {
        let hint = if app.results.is_empty() {
            "type to search"
        } else {
            "loading…"
        };
        frame.render_widget(
            Paragraph::new(hint).block(Block::bordered().title(" detail ")),
            area,
        );
        return;
    };

    let title = match app.pane {
        Pane::Rels => format!(" {} ", detail.title),
        Pane::Impact => format!(" impact — {} ", detail.title),
    };
    let block = bordered(&title, app.focus == Focus::Rels);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [header, list_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(inner);

    let (heading, items): (String, Vec<ListItem>) = match app.pane {
        Pane::Rels => (
            format!("references ({})", detail.total_rels),
            detail
                .rels
                .iter()
                .map(|r| ListItem::new(format!(" ← {:<28} {}", r.name, r.kind)))
                .collect(),
        ),
        Pane::Impact => {
            let rows = detail.impact.as_deref().unwrap_or(&[]);
            let depth = rows.iter().map(|r| r.depth).max().unwrap_or(0);
            (
                format!("impact: {} nodes, depth {depth}", rows.len()),
                rows.iter()
                    .map(|r| ListItem::new(format!(" {} (depth {})", r.label, r.depth)))
                    .collect(),
            )
        }
    };

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(detail.location.clone()).style(Style::new().fg(ACCENT)),
            Line::from(""),
            Line::from(heading),
        ]),
        header,
    );
    let list = List::new(items)
        .highlight_symbol("»")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    // No marker while this pane is unfocused: two visible cursors read as
    // two live selections, and `↵` only ever acts on this one when it holds
    // focus.
    let mut state =
        ListState::default().with_selected((app.focus == Focus::Rels).then_some(app.rel_sel));
    frame.render_stateful_widget(list, list_area, &mut state);
}

fn bordered<'a>(title: &'a str, focused: bool) -> Block<'a> {
    let block = Block::bordered().title(title);
    if focused {
        block.border_style(Style::new().fg(ACCENT))
    } else {
        block
    }
}

fn footer_hint(app: &Explorer) -> String {
    if let Some(message) = &app.message {
        return message.clone();
    }
    match app.focus {
        Focus::Search => " ↑↓ move  ↵ results  esc leave search  ^C quit".into(),
        _ => " ↑↓ move  ↵ open  i impact  esc back  / search  q quit".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(names: &[&str]) -> Vec<SymbolRow> {
        names
            .iter()
            .map(|n| SymbolRow {
                id: format!("sym-{n}"),
                name: (*n).to_string(),
                kind: "class".into(),
            })
            .collect()
    }

    fn detail_with(targets: &[Option<&str>]) -> Detail {
        Detail {
            title: "T (class)".into(),
            location: "a.cs:1".into(),
            rels: targets
                .iter()
                .map(|t| RelRow {
                    target: t.map(str::to_string),
                    name: "Other".into(),
                    kind: "references".into(),
                })
                .collect(),
            total_rels: targets.len(),
            impact: None,
        }
    }

    fn app_with_results(names: &[&str]) -> Explorer {
        let mut app = Explorer::new(Vec::new());
        app.set_results(rows(names));
        app
    }

    #[test]
    fn typing_marks_the_search_dirty_and_backspace_undoes_it() {
        let mut app = Explorer::new(Vec::new());
        for c in "Todo".chars() {
            app.on_key(Key::Char(c));
        }
        assert_eq!(app.query, "Todo");
        assert!(app.search_dirty);

        app.set_results(rows(&["TodoItem"]));
        assert!(!app.search_dirty, "loading results clears the dirty flag");

        app.on_key(Key::Backspace);
        assert_eq!(app.query, "Tod");
        assert!(app.search_dirty);
    }

    #[test]
    fn results_drive_the_detail_panel_and_reset_the_selection() {
        let mut app = Explorer::new(Vec::new());
        app.set_results(rows(&["A", "B", "C"]));
        assert_eq!(app.sel, 0);
        assert_eq!(app.current.as_deref(), Some("sym-A"));
        assert_eq!(app.pending_detail(), Some("sym-A"));

        app.set_detail(detail_with(&[]));
        assert_eq!(app.pending_detail(), None);

        // A new query replaces the list and re-points the detail panel.
        app.set_results(rows(&["Z"]));
        assert_eq!(app.sel, 0);
        assert_eq!(app.current.as_deref(), Some("sym-Z"));
        assert_eq!(app.pending_detail(), Some("sym-Z"));
    }

    #[test]
    fn empty_results_leave_no_current_symbol() {
        let mut app = app_with_results(&["A"]);
        app.set_results(Vec::new());
        assert_eq!(app.current, None);
        assert_eq!(app.pending_detail(), None);
    }

    #[test]
    fn selection_clamps_at_both_ends_of_the_results_list() {
        let mut app = app_with_results(&["A", "B", "C"]);
        app.focus = Focus::Results;

        app.on_key(Key::Up);
        assert_eq!(app.sel, 0, "up at the top is a no-op");

        app.on_key(Key::Down);
        app.on_key(Key::Down);
        app.on_key(Key::Down);
        assert_eq!(app.sel, 2, "down at the bottom is a no-op");
        assert_eq!(app.current.as_deref(), Some("sym-C"));
    }

    #[test]
    fn arrows_move_the_results_list_while_the_search_box_is_focused() {
        let mut app = app_with_results(&["A", "B"]);
        assert_eq!(app.focus, Focus::Search);
        app.on_key(Key::Down);
        assert_eq!(app.sel, 1);
        assert_eq!(app.current.as_deref(), Some("sym-B"));
    }

    #[test]
    fn relationship_selection_clamps_within_the_panel() {
        let mut app = app_with_results(&["A"]);
        app.set_detail(detail_with(&[Some("sym-X"), Some("sym-Y")]));
        app.focus = Focus::Rels;

        app.on_key(Key::Up);
        assert_eq!(app.rel_sel, 0);
        app.on_key(Key::Down);
        app.on_key(Key::Down);
        assert_eq!(app.rel_sel, 1);
    }

    #[test]
    fn entering_a_relationship_pushes_the_back_stack_and_esc_pops_it() {
        let mut app = app_with_results(&["A"]);
        app.set_detail(detail_with(&[Some("sym-X")]));
        app.focus = Focus::Rels;

        app.on_key(Key::Enter);
        assert_eq!(app.current.as_deref(), Some("sym-X"));
        assert_eq!(app.back, vec!["sym-A".to_string()]);
        assert_eq!(app.detail, None, "the new symbol's detail must be reloaded");

        app.set_detail(detail_with(&[Some("sym-Y")]));
        app.on_key(Key::Enter);
        assert_eq!(app.current.as_deref(), Some("sym-Y"));
        assert_eq!(app.back, vec!["sym-A".to_string(), "sym-X".to_string()]);

        app.on_key(Key::Esc);
        assert_eq!(app.current.as_deref(), Some("sym-X"));
        app.on_key(Key::Backspace);
        assert_eq!(app.current.as_deref(), Some("sym-A"));
        assert!(app.back.is_empty());
    }

    #[test]
    fn esc_with_an_empty_back_stack_returns_to_the_search_box() {
        let mut app = app_with_results(&["A"]);
        app.focus = Focus::Results;
        app.on_key(Key::Esc);
        assert_eq!(app.focus, Focus::Search);
        assert_eq!(app.current.as_deref(), Some("sym-A"), "nothing was popped");
    }

    #[test]
    fn an_external_relationship_reports_instead_of_navigating() {
        let mut app = app_with_results(&["A"]);
        app.set_detail(detail_with(&[None]));
        app.focus = Focus::Rels;

        app.on_key(Key::Enter);
        assert_eq!(app.current.as_deref(), Some("sym-A"));
        assert!(app.back.is_empty());
        assert!(app.message.is_some());
    }

    #[test]
    fn moving_the_results_selection_drops_a_stale_back_stack() {
        let mut app = app_with_results(&["A", "B"]);
        app.set_detail(detail_with(&[Some("sym-X")]));
        app.focus = Focus::Rels;
        app.on_key(Key::Enter);
        assert!(!app.back.is_empty());

        app.focus = Focus::Results;
        app.on_key(Key::Down);
        assert!(app.back.is_empty());
        assert_eq!(app.current.as_deref(), Some("sym-B"));
    }

    #[test]
    fn impact_is_only_requested_once_its_pane_is_open() {
        let mut app = app_with_results(&["A"]);
        app.set_detail(detail_with(&[]));
        assert_eq!(app.pending_impact(), None);

        app.focus = Focus::Results;
        app.on_key(Key::Char('i'));
        assert_eq!(app.pane, Pane::Impact);
        assert_eq!(app.pending_impact(), Some("sym-A"));

        app.set_impact(vec![ImpactRow {
            label: "x".into(),
            depth: 1,
        }]);
        assert_eq!(app.pending_impact(), None);

        app.on_key(Key::Char('i'));
        assert_eq!(app.pane, Pane::Rels);
    }

    #[test]
    fn q_quits_outside_the_search_box_but_types_inside_it() {
        let mut app = app_with_results(&["A"]);
        app.on_key(Key::Char('q'));
        assert!(!app.quit);
        assert_eq!(app.query, "q");

        app.focus = Focus::Results;
        app.on_key(Key::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn ctrl_c_quits_from_the_search_box_too() {
        let mut app = Explorer::new(Vec::new());
        app.on_key(Key::Quit);
        assert!(app.quit);
    }

    #[test]
    fn local_name_keeps_only_the_symbol_tail() {
        assert_eq!(local_name("src/db.ts::TodoDbContext"), "TodoDbContext");
        assert_eq!(local_name("TodoItem"), "TodoItem");
    }
}
