//! `cargo xtask fuzz-differential <seconds> <workdir>`: runs the
//! `differential` fuzz target for that long, seeded from the committed
//! vectors, with the corpus and any crash under `<workdir>`.
//!
//! Set `LZMA_SDK` (and on x86-64 Linux `LZMA_ORACLE_ASSEMBLER`) to put the
//! LZMA SDK's own decoders into the comparison; see tools/sdk-oracle/build.rs.
//! `FUZZ_TOOLCHAIN` picks the nightly (default `nightly`).

use std::{
    env, fs,
    path::Path,
    process::{Command, ExitCode},
};

use crate::cmd::repo_root;

/// The target's second byte, picking the largest input and output slices.
const WHOLE: u8 = 0x0F;

pub fn fuzz_differential(mut args: impl Iterator<Item = String>) -> ExitCode {
    let (Some(seconds), Some(work)) = (args.next(), args.next()) else {
        eprintln!("usage: cargo xtask fuzz-differential <seconds> <workdir>");
        return ExitCode::from(2);
    };
    let root = repo_root();
    let work = Path::new(&work);
    let corpus = work.join("corpus");
    let artifacts = work.join("artifacts");
    fs::create_dir_all(&corpus).expect("create corpus directory");
    fs::create_dir_all(&artifacts).expect("create artifacts directory");

    // The target's first byte picks LZMA (even) or LZMA2 (odd, with the
    // dictionary property above the low bit).
    for entry in fs::read_dir(root.join("tests/data")).expect("read tests/data") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "lzma") {
            let mut seed = vec![0, WHOLE];
            seed.extend(fs::read(&path).expect("read vector"));
            fs::write(corpus.join(path.file_name().expect("name")), seed).expect("write seed");
        }
    }
    // decode_lzma2 seeds are <shape> <dictionary property> <stream>.
    for entry in fs::read_dir(root.join("fuzz/corpus/decode_lzma2")).expect("read seeds") {
        let path = entry.expect("entry").path();
        let bytes = fs::read(&path).expect("read seed");
        if bytes.len() < 2 {
            continue;
        }
        let mut seed = vec![((bytes[1] % 25) << 1) | 1, WHOLE];
        seed.extend_from_slice(&bytes[2..]);
        let name = format!(
            "lzma2-{}",
            path.file_name().expect("name").to_string_lossy()
        );
        fs::write(corpus.join(name), seed).expect("write seed");
    }

    let toolchain = env::var("FUZZ_TOOLCHAIN").unwrap_or_else(|_| "nightly".into());
    let mut artifact_prefix = artifacts.display().to_string();
    artifact_prefix.push(std::path::MAIN_SEPARATOR);
    let status = Command::new("cargo")
        .arg(format!("+{toolchain}"))
        .args(["fuzz", "run", "differential"])
        .arg(&corpus)
        .arg("--")
        .arg(format!("-max_total_time={seconds}"))
        .arg("-max_len=65536")
        .arg(format!("-artifact_prefix={artifact_prefix}"))
        .current_dir(root.join("fuzz"))
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("fuzz-differential: cannot run cargo fuzz: {e}");
            ExitCode::FAILURE
        }
    }
}
