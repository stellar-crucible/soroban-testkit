//! `storage-footprint-diff BASE.json HEAD.json [--threshold PCT] [--output FILE] [--allow-missing-base]`
//!
//! Compares two storage footprint artifacts, prints a Markdown report for a
//! pull-request comment, and exits non-zero when a contract grew past the
//! threshold. See `soroban_testkit_core::footprint`.

use std::io::Write as _;
use std::process::ExitCode;

use soroban_testkit_core::footprint::run_diff_cli;

fn main() -> ExitCode {
    let outcome = run_diff_cli(std::env::args().skip(1));
    if !outcome.stdout.is_empty() {
        let _ = std::io::stdout().write_all(outcome.stdout.as_bytes());
    }
    if !outcome.stderr.is_empty() {
        let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    }
    ExitCode::from(outcome.exit_code)
}
