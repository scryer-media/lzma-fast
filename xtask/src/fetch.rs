//! The upstream inputs the trust jobs compare this crate against, fetched
//! pinned: `cargo xtask sdk`, `cargo xtask jwasm` and `cargo xtask xz-tests`.
//!
//! Every one is named by something that cannot move under it. A git
//! dependency is fetched by commit, not by tag or by GitHub's generated
//! archive: a commit id names exactly one tree, a tag can be re-pointed, and
//! an archive's bytes are not promised to stay the same. A release tarball is
//! checked against its SHA-256 before anything is taken out of it.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use crate::{cmd::repo_root, sha256};

/// The LZMA SDK source the decode loops were ported from (tag 26.03).
/// tools/asm-provenance and tools/sdk-oracle pin the SHA-256 of every file
/// they take from it.
const SDK_REPO: &str = "https://github.com/ip7z/7zip.git";
const SDK_COMMIT: &str = "0766b733fe3e06dd2a7f9a3cfbf2108ac73abd17";

/// JWasm, the MASM-compatible assembler the SDK's own Linux build uses for
/// `Asm/x86` (`MY_ASM = jwasm` in `CPP/7zip/7zip_gcc.mak`); tag v2.20, the
/// latest non-prerelease.
const JWASM_REPO: &str = "https://github.com/Baron-von-Riedesel/JWasm.git";
const JWASM_COMMIT: &str = "ac54827ff40b77ecd6e77ed0866a43fc60cd5fe1";

/// XZ Utils, for its decoder test files. The same release, and so the same
/// digest, that `.github/scripts/install-xz.sh` builds `xz` from.
const XZ_VERSION: &str = "5.8.3";
const XZ_SHA256: &str = "3d3a1b973af218114f4f889bbaa2f4c037deaae0c8e815eec381c3d546b974a0";

/// The test files of XZ Utils 5.6.0 and 5.6.1 that carried the CVE-2024-3094
/// payload. No clean release has them; if one ever appears in a tarball this
/// task is pointed at, something is very wrong.
const BACKDOOR_FILES: &[&str] = &["bad-3-corrupt_lzma2.xz", "good-large_compressed.lzma"];

pub fn sdk(mut args: impl Iterator<Item = String>) -> ExitCode {
    let Some(dest) = args.next() else {
        eprintln!("usage: cargo xtask sdk <destination>");
        return ExitCode::from(2);
    };
    result(
        git_at_commit(SDK_REPO, SDK_COMMIT, Path::new(&dest)).map(|()| {
            println!("LZMA SDK at {SDK_COMMIT} in {dest}");
        }),
    )
}

/// Builds JWasm under the given directory and prints the binary's path, for
/// CI to put in `LZMA_ORACLE_ASSEMBLER`. Unix only: it builds with make.
pub fn jwasm(mut args: impl Iterator<Item = String>) -> ExitCode {
    let Some(dest) = args.next() else {
        eprintln!("usage: cargo xtask jwasm <build directory>");
        return ExitCode::from(2);
    };
    let dest = Path::new(&dest);
    result((|| {
        git_at_commit(JWASM_REPO, JWASM_COMMIT, dest)?;
        let jobs = std::thread::available_parallelism().map_or(1, |n| n.get());
        let status = Command::new("make")
            .args(["--silent", "-f", "GccUnix.mak", &format!("-j{jobs}")])
            .current_dir(dest)
            .stdout(std::io::stderr())
            .status()
            .map_err(|e| format!("run make: {e}"))?;
        if !status.success() {
            return Err("building JWasm failed".into());
        }
        let binary = fs::canonicalize(dest.join("build/GccUnixR/jwasm"))
            .map_err(|e| format!("no JWasm binary after the build: {e}"))?;
        println!("{}", binary.display());
        Ok(())
    })())
}

/// Copies XZ Utils' `.xz` and `.lzma` test files into the given directory
/// (default `target/xz-utils-tests`), each checked against
/// `tests/xz-utils.manifest`.
pub fn xz_tests(mut args: impl Iterator<Item = String>) -> ExitCode {
    let root = repo_root();
    let dest = args
        .next()
        .map_or_else(|| root.join("target").join("xz-utils-tests"), PathBuf::from);
    result(xz_tests_into(&root, &dest))
}

fn xz_tests_into(root: &Path, dest: &Path) -> Result<(), String> {
    let manifest = manifest(root)?;
    let work = root.join("target").join("xz-utils-fetch");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;

    let name = format!("xz-{XZ_VERSION}.tar.gz");
    let tarball = work.join(&name);
    let url =
        format!("https://github.com/tukaani-project/xz/releases/download/v{XZ_VERSION}/{name}");
    download(&url, &tarball, XZ_SHA256)?;

    let inner = format!("xz-{XZ_VERSION}/tests/files");
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&name)
        .arg(&inner)
        .current_dir(&work)
        .status()
        .map_err(|e| format!("run tar: {e}"))?;
    if !status.success() {
        return Err(format!("tar could not extract {inner}"));
    }

    let extracted = work.join(&inner);
    let mut found = Vec::new();
    for entry in fs::read_dir(&extracted).map_err(|e| e.to_string())? {
        let file = entry.map_err(|e| e.to_string())?.file_name();
        let file = file.to_string_lossy().into_owned();
        if BACKDOOR_FILES.contains(&file.as_str()) {
            return Err(format!(
                "{file} is in the tarball: it is one of the files that carried the \
                 CVE-2024-3094 payload, and no release this task should fetch has it"
            ));
        }
        if file.ends_with(".xz") || file.ends_with(".lzma") {
            found.push(file);
        }
    }
    found.sort();
    let listed: Vec<&String> = manifest.keys().collect();
    if found.iter().collect::<Vec<_>>() != listed {
        let extra: Vec<_> = found
            .iter()
            .filter(|f| !manifest.contains_key(*f))
            .collect();
        let missing: Vec<_> = listed.iter().filter(|f| !found.contains(f)).collect();
        return Err(format!(
            "the tarball's test files are not the manifest's: not listed {extra:?}, absent {missing:?}"
        ));
    }

    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for (file, digest) in &manifest {
        let bytes = fs::read(extracted.join(file)).map_err(|e| format!("read {file}: {e}"))?;
        let actual = sha256::hex(&bytes);
        if &actual != digest {
            return Err(format!(
                "{file}: SHA-256 {actual}, the manifest says {digest}"
            ));
        }
        fs::write(dest.join(file), bytes).map_err(|e| format!("write {file}: {e}"))?;
    }
    let _ = fs::remove_dir_all(&work);
    println!(
        "xz-tests: {} files from XZ Utils {XZ_VERSION} in {}",
        manifest.len(),
        dest.display()
    );
    Ok(())
}

/// File name to SHA-256, from `tests/xz-utils.manifest`.
fn manifest(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = root.join("tests").join("xz-utils.manifest");
    let text = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [digest, _, _, file] = fields[..] else {
            return Err(format!("manifest line is not four fields: {line}"));
        };
        out.insert(file.to_owned(), digest.to_owned());
    }
    Ok(out)
}

/// Fetches `commit` of `repo` into a fresh repository at `dest`, and proves
/// that is what was checked out.
fn git_at_commit(repo: &str, commit: &str, dest: &Path) -> Result<(), String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(dest)
            .args(args)
            .output()
            .map_err(|e| format!("run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    git(&["init", "--quiet"])?;
    git(&["fetch", "--quiet", "--depth", "1", repo, commit])?;
    git(&[
        "-c",
        "advice.detachedHead=false",
        "checkout",
        "--quiet",
        "FETCH_HEAD",
    ])?;
    let head = git(&["rev-parse", "HEAD"])?;
    if head != commit {
        return Err(format!("{repo}: checked out {head}, wanted {commit}"));
    }
    Ok(())
}

/// Downloads `url` to `to` with curl, which ships with every OS CI runs on,
/// and fails unless the bytes have the given SHA-256.
fn download(url: &str, to: &Path, digest: &str) -> Result<(), String> {
    let status = Command::new("curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
        ])
        .arg("--output")
        .arg(to)
        .arg(url)
        .status()
        .map_err(|e| format!("run curl: {e}"))?;
    if !status.success() {
        return Err(format!("download of {url} failed"));
    }
    let bytes = fs::read(to).map_err(|e| e.to_string())?;
    let actual = sha256::hex(&bytes);
    if actual != digest {
        return Err(format!("{url}: SHA-256 {actual}, pinned {digest}"));
    }
    Ok(())
}

fn result(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}
