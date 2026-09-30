//! The `rk` command line: fetch pinned Linux trees, build them with rucc and with a reference
//! compiler, boot and test what comes out, and record what happened.

use std::process::ExitCode;

const USAGE: &str = "\
usage: rk <command> [options]

commands:
  help       print this text
  version    print the version of rk

Each milestone of the plan in docs/plan adds commands here. See docs/plan/13-rucc-kernel.md for
the full list.
";

fn main() -> ExitCode {
    let words: Vec<String> = std::env::args().skip(1).collect();
    match words.first().map(String::as_str) {
        None | Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some("version" | "--version") => {
            println!("rk {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("rk: unknown command {other}; try rk help");
            ExitCode::from(2)
        }
    }
}
