//! CodeKurve CLI binary (composition root).

use std::process::ExitCode;

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("version") => {
            println!("codekurve {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: codekurve version");
            ExitCode::from(2)
        }
    }
}
