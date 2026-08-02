//! Interactive agent picker for no-arg `codekurve install`.
//!
//! ```text
//! ┌─ codekurve install ────────────────────────┐
//! │ Detected agents:                        │
//! │                                            │
//! │   [x] claude-code   .mcp.json              │
//! │   [x] cursor        .cursor/mcp.json       │
//! │ > [ ] codex-cli     ~/.codex/config.toml   │
//! │   [x] copilot       .vscode/mcp.json       │
//! │   [x] opencode      opencode.json          │
//! └────────────────────────────────────────────┘
//!  space toggle  ↵ install  q cancel
//! ```
//!
//! Undetected clients are **shown disabled** rather than hidden: a user who
//! expects their agent in the list needs to see that CodeKurve supports it
//! and simply did not find it on this machine — an invisible row looks like
//! missing support. They cannot be checked (detection is the whole premise
//! of the no-arg form); `codekurve install <client>` remains the explicit
//! override for a client installed somewhere unusual.
//!
//! Every non-interactive path is untouched: `main.rs` only reaches this
//! screen when there is no client argument, `--yes` was not passed, **and**
//! stdin is a terminal. `--yes`, a piped stdin and `install <client>` all
//! still go straight to `install::run`.

use std::path::Path;

use codekurve::install::{self, ClientPlan};

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::{read_key, step, Key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub path: String,
    pub detected: bool,
    pub checked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Install,
    Cancel,
}

pub struct Picker {
    pub rows: Vec<Row>,
    pub sel: usize,
    pub done: Option<Outcome>,
}

impl Picker {
    /// Detected clients start checked (the no-arg form's existing default
    /// was "configure everything detected"); undetected ones start
    /// unchecked and stay that way.
    pub fn new(plan: Vec<ClientPlan>) -> Self {
        let rows = plan
            .into_iter()
            .map(|p| Row {
                name: p.name.to_string(),
                path: p.config_path.display().to_string(),
                detected: p.detected,
                checked: p.detected,
            })
            .collect();
        Self {
            rows,
            sel: 0,
            done: None,
        }
    }

    pub fn checked(&self) -> Vec<&str> {
        self.rows
            .iter()
            .filter(|r| r.checked)
            .map(|r| r.name.as_str())
            .collect()
    }

    fn toggle(&mut self) {
        if let Some(row) = self.rows.get_mut(self.sel) {
            if row.detected {
                row.checked = !row.checked;
            }
        }
    }

    pub fn on_key(&mut self, key: Key) {
        match key {
            Key::Up => self.sel = step(self.rows.len(), self.sel, false),
            Key::Down => self.sel = step(self.rows.len(), self.sel, true),
            Key::Char(' ') => self.toggle(),
            Key::Enter => self.done = Some(Outcome::Install),
            Key::Char('q') | Key::Esc | Key::Quit => self.done = Some(Outcome::Cancel),
            _ => {}
        }
    }
}

/// The interactive branch of no-arg `codekurve install`.
///
/// With nothing detected there is nothing to pick, so this defers to
/// `install::run`, which already produces the "no supported MCP client
/// detected" error plus manual instructions — one wording, not two.
pub fn run(root: &Path) -> Result<(), String> {
    let plan = install::plan(root)?;
    if !plan.iter().any(|p| p.detected) {
        return install::run(root, None, false);
    }

    let mut app = Picker::new(plan);
    let mut terminal = ratatui::try_init().map_err(|e| e.to_string())?;
    let outcome = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    outcome?;

    match app.done {
        Some(Outcome::Install) => install::install_named(root, &app.checked()),
        _ => {
            println!("aborted; no changes made.");
            Ok(())
        }
    }
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut Picker) -> Result<(), String> {
    while app.done.is_none() {
        terminal
            .draw(|frame| render(frame, app))
            .map_err(|e| e.to_string())?;
        if let Some(key) = read_key()? {
            app.on_key(key);
        }
    }
    Ok(())
}

fn render(frame: &mut Frame, app: &Picker) {
    let [body, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(frame.area());

    let block = Block::bordered().title(" codekurve install ");
    let inner = block.inner(body);
    frame.render_widget(block, body);

    let [heading, list_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
    frame.render_widget(Paragraph::new("Detected agents:"), heading);

    let width = app.rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .map(|r| {
            let mark = if r.checked { 'x' } else { ' ' };
            let text = format!(
                "[{mark}] {:<width$}   {}{}",
                r.name,
                r.path,
                if r.detected { "" } else { "   (not detected)" }
            );
            let item = ListItem::new(text);
            if r.detected {
                item
            } else {
                item.style(Style::new().fg(Color::DarkGray))
            }
        })
        .collect();

    let list = List::new(items)
        .highlight_symbol("> ")
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default().with_selected(Some(app.sel));
    frame.render_stateful_widget(list, list_area, &mut state);

    frame.render_widget(
        Paragraph::new(" space toggle  ↵ install  q cancel")
            .style(Style::new().fg(Color::DarkGray)),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn plan(entries: &[(&'static str, bool)]) -> Vec<ClientPlan> {
        entries
            .iter()
            .map(|(name, detected)| ClientPlan {
                name,
                config_path: PathBuf::from(format!("/p/{name}.json")),
                scope: "project scope",
                detected: *detected,
            })
            .collect()
    }

    fn picker() -> Picker {
        Picker::new(plan(&[
            ("claude-code", true),
            ("cursor", false),
            ("codex-cli", true),
        ]))
    }

    #[test]
    fn detected_clients_start_checked_and_undetected_ones_do_not() {
        let app = picker();
        assert_eq!(app.checked(), vec!["claude-code", "codex-cli"]);
    }

    #[test]
    fn space_toggles_the_highlighted_detected_row() {
        let mut app = picker();
        app.on_key(Key::Char(' '));
        assert_eq!(app.checked(), vec!["codex-cli"]);
        app.on_key(Key::Char(' '));
        assert_eq!(app.checked(), vec!["claude-code", "codex-cli"]);
    }

    #[test]
    fn an_undetected_row_cannot_be_checked() {
        let mut app = picker();
        app.on_key(Key::Down);
        assert_eq!(app.sel, 1);
        app.on_key(Key::Char(' '));
        assert_eq!(app.checked(), vec!["claude-code", "codex-cli"]);
    }

    #[test]
    fn selection_clamps_at_both_ends() {
        let mut app = picker();
        app.on_key(Key::Up);
        assert_eq!(app.sel, 0);
        for _ in 0..5 {
            app.on_key(Key::Down);
        }
        assert_eq!(app.sel, 2);
    }

    #[test]
    fn enter_installs_and_q_or_esc_cancels() {
        let mut app = picker();
        app.on_key(Key::Enter);
        assert_eq!(app.done, Some(Outcome::Install));

        let mut app = picker();
        app.on_key(Key::Char('q'));
        assert_eq!(app.done, Some(Outcome::Cancel));

        let mut app = picker();
        app.on_key(Key::Esc);
        assert_eq!(app.done, Some(Outcome::Cancel));

        let mut app = picker();
        app.on_key(Key::Quit);
        assert_eq!(app.done, Some(Outcome::Cancel));
    }

    #[test]
    fn unchecking_everything_yields_an_empty_install_set() {
        let mut app = picker();
        app.on_key(Key::Char(' '));
        app.on_key(Key::Down);
        app.on_key(Key::Down);
        app.on_key(Key::Char(' '));
        assert!(app.checked().is_empty());
    }
}
