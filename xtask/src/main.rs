//! Repository tasks, run as `cargo xtask <task>`. Plain Rust with no
//! dependencies, so every task runs wherever `cargo` does.

mod asm_lab;
mod cmd;
mod fixtures;
mod hooks;
mod release;
mod rng;

use std::{env, process::ExitCode};

const TASKS: &str = "\
cargo xtask release     cut a release; see `cargo xtask release --help`
cargo xtask fixtures    generate the benchmark fixtures under bench/fixtures
cargo xtask pre-commit  what .githooks/pre-commit runs
cargo xtask asm-lab     build and verify the asm-lab decode-loop variants";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("release") => release::release(args),
        Some("fixtures") => fixtures::fixtures(),
        Some("pre-commit") => hooks::pre_commit(),
        Some("asm-lab") => asm_lab::asm_lab(args),
        Some("-h" | "--help") | None => {
            println!("{TASKS}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("xtask: unknown task '{other}'\n\n{TASKS}");
            ExitCode::from(2)
        }
    }
}
