//! Repository tasks, run as `cargo xtask <task>`. Plain Rust with no
//! dependencies, so every task runs wherever `cargo` does.

mod asm_lab;
mod cmd;
mod fetch;
mod fixtures;
mod fuzz;
mod hooks;
mod release;
mod rng;
mod vectors;

use std::{env, process::ExitCode};

const TASKS: &str = "\
cargo xtask release     cut a release; see `cargo xtask release --help`
cargo xtask fixtures    generate the benchmark fixtures under bench/fixtures
cargo xtask pre-commit  what .githooks/pre-commit runs
cargo xtask asm-lab     build and verify the asm-lab decode-loop variants
cargo xtask vectors     regenerate tests/data and the fuzz seeds; --check compares
cargo xtask sdk <dir>   fetch the pinned LZMA SDK source the decode loops were ported from
cargo xtask jwasm <dir> build the pinned JWasm assembler and print its path (Unix)
cargo xtask xz-tests    fetch XZ Utils' decoder test files, checked against tests/xz-utils.manifest
cargo xtask lzma-util [dir]
                        build the pinned SDK's reference LZMA encoder for the parity tests
cargo xtask fuzz-differential <seconds> <workdir>
                        fuzz lzma-turbo against the LZMA SDK, seeded from the vectors";

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("release") => release::release(args),
        Some("fixtures") => fixtures::fixtures(),
        Some("pre-commit") => hooks::pre_commit(),
        Some("asm-lab") => asm_lab::asm_lab(args),
        Some("vectors") => vectors::vectors(args),
        Some("sdk") => fetch::sdk(args),
        Some("jwasm") => fetch::jwasm(args),
        Some("xz-tests") => fetch::xz_tests(args),
        Some("lzma-util") => fetch::lzma_util(args),
        Some("fuzz-differential") => fuzz::fuzz_differential(args),
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
