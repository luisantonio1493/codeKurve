//! `codekurve update` and the binary-removal path of `codekurve uninstall`
//! — the only two places CodeKurve ever spawns a subprocess, and the only two
//! paths by which a network request happens on CodeKurve's behalf.
//!
//! See `docs/adr/0012-update-via-install-script.md` for the full rationale.
//! In short: CodeKurve gains **no HTTP client dependency** and makes no
//! network call from Rust. Both commands hand off to the already-published
//! install script (`install.sh` / `install.ps1`), which is what downloads the
//! release binary. ADR 0005 ("no outbound network requests, in any mode")
//! keeps its substance for every analysis path — `index`, `watch`, `mcp`,
//! `tui`, and every query command are untouched and still spawn nothing.
//!
//! Both paths are reachable *only* by a user explicitly typing the command;
//! nothing dispatches here automatically, and there is no update check.
//!
//! The two safety rules that differ from `install.rs`'s `confirm` helper are
//! deliberate and enforced in [`decide`].

use std::io::{IsTerminal, Write};
use std::process::Command;

/// The install-script URLs `README.md` documents. Defined here so README and
/// code cannot drift silently — if these move, this constant is the one place
/// the CLI reads them from.
pub const INSTALL_SH_URL: &str =
    "https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.sh";
pub const INSTALL_PS1_URL: &str =
    "https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1";

/// Which install-script operation to run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Download and replace the codekurve executable (`codekurve update`).
    Update,
    /// Delete the codekurve executable (`codekurve uninstall --binary`).
    RemoveBinary,
}

/// A fully-resolved subprocess invocation: exactly what will be executed,
/// available for printing *before* anything runs.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    pub program: &'static str,
    pub args: Vec<String>,
}

impl Plan {
    /// The command as a human would type it — this exact string is printed to
    /// the user before the confirmation prompt.
    pub fn command_line(&self) -> String {
        let mut out = String::from(self.program);
        for arg in &self.args {
            // Only the trailing script body needs quoting; a plain flag does
            // not, and quoting it would misrepresent what is executed.
            if arg.contains(' ') {
                out.push_str(&format!(" \"{arg}\""));
            } else {
                out.push(' ');
                out.push_str(arg);
            }
        }
        out
    }
}

/// Builds the invocation. Pure: platform in, program + argument vector out,
/// so both platform shapes are unit-testable on any host (a test may not
/// spawn the installer or touch the network).
///
/// `windows == false` is the POSIX shape; the two `sh -s --` dashes are what
/// `install.sh` itself documents for passing `--uninstall` through a pipe.
pub fn plan(op: Op, windows: bool) -> Plan {
    if windows {
        let script = match op {
            Op::Update => format!("irm {INSTALL_PS1_URL} | iex"),
            // `irm | iex` cannot pass arguments; the scriptblock form is the
            // standard PowerShell idiom for a parameterised piped script.
            Op::RemoveBinary => {
                format!("&([scriptblock]::Create((irm {INSTALL_PS1_URL}))) -Uninstall")
            }
        };
        Plan {
            program: "powershell",
            args: vec!["-NoProfile".to_string(), "-Command".to_string(), script],
        }
    } else {
        let script = match op {
            Op::Update => format!("curl -fsSL {INSTALL_SH_URL} | sh"),
            Op::RemoveBinary => format!("curl -fsSL {INSTALL_SH_URL} | sh -s -- --uninstall"),
        };
        Plan {
            program: "sh",
            args: vec!["-c".to_string(), script],
        }
    }
}

/// Whether to proceed, given `--yes` and whether stdin is a terminal.
///
/// Deliberately *not* `install.rs`'s `confirm`: that helper auto-proceeds on a
/// non-terminal stdin, which is right for `install`/`uninstall` because they
/// only write local config files (and back them up first). These two commands
/// download and replace — or delete — an executable. Silently doing that in a
/// scripted context is a materially worse failure mode than a hung prompt, so
/// a non-terminal stdin *refuses* here and tells the user to pass `--yes`.
/// `--yes` remains the explicit opt-in for automation.
///
/// Split out from [`run`] so the refusal is testable without a real terminal.
fn decide(op: Op, yes: bool, is_terminal: bool) -> Result<bool, String> {
    if yes {
        return Ok(true);
    }
    refuse_without_terminal(op, is_terminal)?;
    print!("run this? [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// The non-terminal refusal on its own, so `install::uninstall --binary` can
/// fail *before* it starts editing agent configs rather than cleaning them and
/// then refusing halfway through.
pub fn refuse_without_terminal(op: Op, is_terminal: bool) -> Result<(), String> {
    if is_terminal {
        return Ok(());
    }
    let what = match op {
        Op::Update => "downloads and replaces the codekurve executable",
        Op::RemoveBinary => "deletes the codekurve executable",
    };
    Err(format!(
        "refusing to run without a terminal to confirm at: this {what}. \
         Re-run with --yes to proceed non-interactively."
    ))
}

/// `install::uninstall`'s early guard: same rule, checked up front.
pub fn precheck_binary_removal(yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    refuse_without_terminal(Op::RemoveBinary, std::io::stdin().is_terminal())
}

/// `codekurve update [--yes]`.
pub fn run(yes: bool) -> Result<(), String> {
    execute(Op::Update, yes)
}

/// The `--binary` half of `codekurve uninstall`. Opt-in only — see
/// `install::uninstall`.
pub fn remove_binary(yes: bool) -> Result<(), String> {
    execute(Op::RemoveBinary, yes)
}

/// Print the exact command, confirm, spawn, propagate the child's status.
fn execute(op: Op, yes: bool) -> Result<(), String> {
    let plan = plan(op, cfg!(windows));
    match op {
        Op::Update => println!(
            "codekurve update runs the published install script, which downloads the latest\n\
             release binary and replaces this one. codekurve itself makes no network request.\n\
             \n\
             about to run:\n  {}\n",
            plan.command_line()
        ),
        Op::RemoveBinary => println!(
            "this will delete the codekurve executable.\n\
             \n\
             about to run:\n  {}\n",
            plan.command_line()
        ),
    }

    if !decide(op, yes, std::io::stdin().is_terminal())? {
        println!("aborted; nothing was run.");
        return Ok(());
    }

    let status = Command::new(plan.program)
        .args(&plan.args)
        .status()
        .map_err(|e| format!("could not run {}: {e}", plan.program))?;
    if !status.success() {
        return Err(format!(
            "the install script failed ({status}); codekurve was not {}.",
            match op {
                Op::Update => "updated",
                Op::RemoveBinary => "removed",
            }
        ));
    }

    if op == Op::Update {
        // Learned the hard way: Cursor and Codex both kept serving from the
        // pre-update binary because their MCP child process was still alive.
        println!(
            "\nDone. If an MCP client (Cursor, Codex, Claude Code, ...) has a codekurve mcp\n\
             server running, it keeps the OLD binary until that client restarts it."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // No test here spawns anything or touches the network: `plan` is pure and
    // `decide` takes the terminal decision as a parameter.

    #[test]
    fn unix_update_command_is_exact() {
        let p = plan(Op::Update, false);
        assert_eq!(p.program, "sh");
        assert_eq!(
            p.args,
            vec![
                "-c".to_string(),
                "curl -fsSL https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.sh | sh".to_string(),
            ]
        );
    }

    #[test]
    fn unix_remove_binary_command_is_exact() {
        let p = plan(Op::RemoveBinary, false);
        assert_eq!(p.program, "sh");
        assert_eq!(
            p.args,
            vec![
                "-c".to_string(),
                "curl -fsSL https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.sh | sh -s -- --uninstall".to_string(),
            ]
        );
    }

    #[test]
    fn windows_update_command_is_exact() {
        let p = plan(Op::Update, true);
        assert_eq!(p.program, "powershell");
        assert_eq!(
            p.args,
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "irm https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1 | iex".to_string(),
            ]
        );
    }

    #[test]
    fn windows_remove_binary_command_is_exact() {
        let p = plan(Op::RemoveBinary, true);
        assert_eq!(p.program, "powershell");
        assert_eq!(
            p.args,
            vec![
                "-NoProfile".to_string(),
                "-Command".to_string(),
                "&([scriptblock]::Create((irm https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.ps1))) -Uninstall".to_string(),
            ]
        );
    }

    #[test]
    fn command_line_is_printable_verbatim() {
        assert_eq!(
            plan(Op::Update, false).command_line(),
            "sh -c \"curl -fsSL https://raw.githubusercontent.com/luisantonio1493/codeKurve/main/install.sh | sh\""
        );
    }

    #[test]
    fn non_terminal_without_yes_refuses() {
        for op in [Op::Update, Op::RemoveBinary] {
            let err = decide(op, false, false).unwrap_err();
            assert!(err.contains("--yes"), "message must name the opt-in: {err}");
        }
    }

    #[test]
    fn yes_proceeds_without_a_terminal() {
        assert!(decide(Op::Update, true, false).unwrap());
        assert!(decide(Op::RemoveBinary, true, false).unwrap());
    }

    #[test]
    fn uninstall_precheck_refuses_before_any_config_is_touched() {
        assert!(refuse_without_terminal(Op::RemoveBinary, false).is_err());
        assert!(refuse_without_terminal(Op::RemoveBinary, true).is_ok());
    }
}
