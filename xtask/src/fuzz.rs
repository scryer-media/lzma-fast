//! `cargo xtask fuzz <seconds> <workdir>`: the fuzz targets CI runs, seeded
//! from the committed vectors, with each target's corpus and any finding under
//! `<workdir>`.
//!
//! The budget is *split* between the targets rather than given to each: a
//! caller asking for half an hour gets half an hour however many targets this
//! file grows, so the workflow's timeout keeps meaning what it says.
//!
//! Set `LZMA_SDK` (and on x86-64 Linux `LZMA_ORACLE_ASSEMBLER`) to put the
//! LZMA SDK's own decoders into the comparison; see tools/sdk-oracle/build.rs.
//! `FUZZ_TOOLCHAIN` picks the nightly (default `nightly`).
//!
//! `cargo xtask fuzz-check` builds every target and runs none, so a target
//! cannot rot between fuzzing runs.

use std::{
    env, fs,
    path::Path,
    process::{Command, ExitCode},
};

use crate::cmd::repo_root;

/// The `differential` target's second byte, picking the largest input and
/// output slices.
const WHOLE: u8 = 0x0F;

/// Every target a fuzzing run covers.
///
/// `differential` compares the decode loops with the C's; `encode_round_trip`
/// asks the question the C cannot answer, which is that whatever this encoder
/// produces, this crate's own decoders return.
const TARGETS: &[&str] = &["differential", "encode_round_trip"];

/// The shortest slot worth starting: below this libFuzzer spends it loading
/// the corpus.
const MIN_SECONDS: u64 = 30;

pub fn fuzz(mut args: impl Iterator<Item = String>) -> ExitCode {
    let (Some(seconds), Some(work)) = (args.next(), args.next()) else {
        eprintln!("usage: cargo xtask fuzz <seconds> <workdir>");
        return ExitCode::from(2);
    };
    let Ok(seconds) = seconds.parse::<u64>() else {
        eprintln!("xtask fuzz: <seconds> must be a number");
        return ExitCode::from(2);
    };
    let root = repo_root();
    let work = Path::new(&work);
    let artifacts = work.join("artifacts");
    if let Err(e) = fs::create_dir_all(&artifacts) {
        eprintln!("xtask fuzz: create {}: {e}", artifacts.display());
        return ExitCode::FAILURE;
    }

    let each = (seconds / TARGETS.len() as u64).max(MIN_SECONDS);
    for target in TARGETS {
        let corpus = work.join(target);
        if let Err(e) = fs::create_dir_all(&corpus) {
            eprintln!("xtask fuzz: create {}: {e}", corpus.display());
            return ExitCode::FAILURE;
        }
        if let Err(e) = seed(&root, target, &corpus) {
            eprintln!("xtask fuzz: seeding {target}: {e}");
            return ExitCode::FAILURE;
        }
        println!("== {target}: {each}s");
        match run(&root, target, &corpus, &artifacts, each) {
            Ok(true) => {}
            Ok(false) => return ExitCode::FAILURE,
            Err(e) => {
                eprintln!("xtask fuzz: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

/// `cargo xtask fuzz-check`: every target compiled, none run.
///
/// A fuzz target is not built by `cargo test`, `cargo clippy` or `cargo doc` -
/// `fuzz/` is its own workspace - so an API change can leave one uncompilable
/// and nothing says so until the next fuzzing run.
pub fn fuzz_check(_args: impl Iterator<Item = String>) -> ExitCode {
    let root = repo_root();
    let toolchain = env::var("FUZZ_TOOLCHAIN").unwrap_or_else(|_| "nightly".into());
    let status = Command::new("cargo")
        .arg(format!("+{toolchain}"))
        // `--dev` is the unoptimized build: this proves the targets compile
        // and link, and nothing here is going to be run.
        .args(["fuzz", "build", "--dev"])
        .current_dir(root.join("fuzz"))
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("fuzz-check: every target builds");
            ExitCode::SUCCESS
        }
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("xtask fuzz-check: cannot run cargo fuzz: {e}");
            ExitCode::FAILURE
        }
    }
}

fn seed(root: &Path, target: &str, corpus: &Path) -> Result<(), String> {
    match target {
        "differential" => seed_differential(root, corpus),
        "encode_round_trip" | "encode_differential" => seed_encode(root, corpus),
        other => Err(format!("no seeds defined for {other}")),
    }
}

fn seed_differential(root: &Path, corpus: &Path) -> Result<(), String> {
    // The target's first byte picks LZMA (even) or LZMA2 (odd, with the
    // dictionary property above the low bit).
    let dir = root.join("tests/data");
    for path in entries(&dir)? {
        if path.extension().is_some_and(|e| e == "lzma") {
            let mut seed = vec![0, WHOLE];
            seed.extend(fs::read(&path).map_err(|e| e.to_string())?);
            write_seed(corpus, &name_of(&path), &seed)?;
        }
    }
    // decode_lzma2 seeds are <shape> <dictionary property> <stream>.
    let dir = root.join("fuzz/corpus/decode_lzma2");
    for path in entries(&dir)? {
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        if bytes.len() < 2 {
            continue;
        }
        let mut seed = vec![((bytes[1] % 25) << 1) | 1, WHOLE];
        seed.extend_from_slice(&bytes[2..]);
        write_seed(corpus, &format!("lzma2-{}", name_of(&path)), &seed)?;
    }
    Ok(())
}

/// The sources `cargo xtask vectors` generates the decoder vectors *from*,
/// each behind the four-byte settings prefix the encode targets read.
///
/// An encoder wants plausible input, not a compressed stream: `src_*.bin` are
/// the plain bytes - text, random, zeros, a mixture - already in the
/// repository with a generator that explains every one.
fn seed_encode(root: &Path, corpus: &Path) -> Result<(), String> {
    // Four prefixes, so each source arrives at more than one setting: a
    // hash-chain finder and a binary-tree one, a fast level and an optimal
    // one. The fourth byte is the encode targets' container and thread
    // selector.
    const PREFIXES: [(&str, [u8; 4]); 4] = [
        ("hc4-l1", [0, 4, 1, 0]),
        ("bt4-l9", [4, 6, 9, 0x5A]),
        ("bt2-l5", [2, 0, 5, 0xA5]),
        ("bt5-l0", [5, 7, 0, 0xFF]),
    ];
    let dir = root.join("tests/data");
    for path in entries(&dir)? {
        if !name_of(&path).starts_with("src_") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        for (tag, cfg) in PREFIXES {
            let mut seed = cfg.to_vec();
            seed.extend_from_slice(&bytes);
            write_seed(corpus, &format!("{tag}-{}", name_of(&path)), &seed)?;
        }
    }
    Ok(())
}

fn entries(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))? {
        out.push(entry.map_err(|e| e.to_string())?.path());
    }
    out.sort();
    Ok(out)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn write_seed(corpus: &Path, name: &str, bytes: &[u8]) -> Result<(), String> {
    fs::write(corpus.join(name), bytes).map_err(|e| format!("write seed {name}: {e}"))
}

/// Runs one target; `Ok(false)` is a finding.
fn run(
    root: &Path,
    target: &str,
    corpus: &Path,
    artifacts: &Path,
    seconds: u64,
) -> Result<bool, String> {
    let toolchain = env::var("FUZZ_TOOLCHAIN").unwrap_or_else(|_| "nightly".into());
    // One directory for every target's findings, so the workflow's
    // upload-on-failure step needs no per-target path.
    let mut artifact_prefix = artifacts.display().to_string();
    artifact_prefix.push(std::path::MAIN_SEPARATOR);
    let status = Command::new("cargo")
        .arg(format!("+{toolchain}"))
        .args(["fuzz", "run", target])
        .arg(corpus)
        .arg("--")
        .arg(format!("-max_total_time={seconds}"))
        .arg("-max_len=65536")
        .arg(format!("-artifact_prefix={artifact_prefix}"))
        .current_dir(root.join("fuzz"))
        .status()
        .map_err(|e| format!("cannot run cargo fuzz: {e}"))?;
    Ok(status.success())
}
