//! CodeKurve's terminal UI (ADR 0011): two screens, no new query logic.
//!
//! * [`explorer`] — an interactive code-graph browser driven entirely by
//!   `codekurve::query` (`search`/`get_symbol`/`relationships`/`impact`).
//!   It never opens SQLite, never builds a statement, and never reimplements
//!   symbol resolution; every panel renders a value that layer already
//!   returns to the CLI and to MCP.
//! * [`picker`] — a checkbox front-end for no-arg `codekurve install`, built
//!   on `codekurve::install`'s `plan`/`install_named`. Config-file shapes,
//!   detection signals and the writers stay in that module.
//!
//! Both screens keep their state in a plain struct with no `ratatui` or
//! terminal types in it, so the state machines are unit-testable without a
//! pty (see each module's `tests`). Rendering and the event loop are the
//! thin shell around them.
//!
//! Terminal restoration goes through `ratatui::try_init`, which installs a
//! panic hook that leaves the alternate screen and disables raw mode before
//! the panic propagates — a crash never leaves the user's shell unusable.

pub mod explorer;
pub mod picker;

pub use explorer::run as run_explorer;
pub use picker::run as run_picker;

/// Keys both screens understand, decoupled from `crossterm::KeyCode` so the
/// state machines can be driven from a test without a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Up,
    Down,
    Left,
    Right,
    Enter,
    Tab,
    Esc,
    /// `Ctrl-C` — quits from anywhere, including while typing in a text
    /// field where a bare `q` is just a character.
    Quit,
}

/// Blocks for the next key press. `Ok(None)` for an event this UI ignores
/// (resize, mouse, key release), which the caller treats as "redraw".
fn read_key() -> Result<Option<Key>, String> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

    let Event::Key(e) = event::read().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if e.kind != KeyEventKind::Press {
        return Ok(None);
    }
    if e.modifiers.contains(KeyModifiers::CONTROL) && matches!(e.code, KeyCode::Char('c' | 'C')) {
        return Ok(Some(Key::Quit));
    }
    Ok(Some(match e.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
        KeyCode::Esc => Key::Esc,
        _ => return Ok(None),
    }))
}

/// One step up or down a list of `len` items, clamped at both ends — no
/// wraparound (a selection that silently jumps from the last row to the
/// first reads as a lost keypress). `len == 0` always stays at 0.
fn step(len: usize, index: usize, down: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if down {
        (index + 1).min(len - 1)
    } else {
        index.saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::step;

    #[test]
    fn step_clamps_at_both_boundaries() {
        assert_eq!(step(3, 0, false), 0, "up at the top stays put");
        assert_eq!(step(3, 2, true), 2, "down at the bottom stays put");
        assert_eq!(step(3, 0, true), 1);
        assert_eq!(step(3, 2, false), 1);
    }

    #[test]
    fn step_on_empty_list_is_always_zero() {
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(step(0, 0, false), 0);
    }
}
