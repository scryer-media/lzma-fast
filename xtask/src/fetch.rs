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

use crate::cmd::repo_root;

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
        let actual = sha256_hex(&bytes);
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
    // The files are checked against pinned SHA-256s, so they must be the
    // committed bytes. Windows runners set core.autocrlf, which would rewrite
    // every LF on checkout; `* -text` in info/attributes outranks both that
    // and the fetched tree's own .gitattributes.
    git(&["config", "core.autocrlf", "false"])?;
    fs::write(dest.join(".git/info/attributes"), "* -text\n").map_err(|e| e.to_string())?;
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
    let actual = sha256_hex(&bytes);
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

/// The reference LZMA encoder, for the bit-exactness tests.
///
/// Builds two binaries from the pinned SDK into `target/lzma-util/`:
///
/// * `lzma`, `C/Util/Lzma/LzmaUtil.c` exactly as it ships, which encodes with
///   `LzmaEncProps_Init`'s defaults;
/// * `lzma-oracle`, the small harness below, which takes every setting
///   `tests/lzma_parity.rs` varies. `LzmaUtil` cannot: it never calls
///   `LzmaEnc_SetProps` with anything but the defaults, so on its own it
///   would pin one level and one match finder;
/// * `lzma2-oracle`, the same for `C/Lzma2Enc.c`, pinned to one solid block
///   and one thread, which is the shape this port has.
///
/// Both are built with `-DZ7_ST`, which is what selects the single-threaded
/// `LzFind.c` over `LzFindMt.c` — the same choice this port made.
pub fn lzma_util(mut args: impl Iterator<Item = String>) -> ExitCode {
    let dest = args
        .next()
        .map_or_else(|| repo_root().join("target/lzma-util"), PathBuf::from);
    result(build_lzma_util(&dest).map(|built| {
        for path in built {
            println!("{}", path.display());
        }
    }))
}

fn build_lzma_util(dest: &Path) -> Result<Vec<PathBuf>, String> {
    let sdk = dest.join("sdk");
    if !sdk.join("C/LzmaEnc.c").is_file() {
        git_at_commit(SDK_REPO, SDK_COMMIT, &sdk)?;
    }
    let c = sdk.join("C");

    let write_src = |name: &str, body: &str| -> Result<PathBuf, String> {
        let path = dest.join(name);
        fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
        Ok(path)
    };
    let oracle_src = write_src("lzma-oracle.c", ORACLE_C)?;
    let oracle2_src = write_src("lzma2-oracle.c", ORACLE2_C)?;
    let filter_src = write_src("filter-oracle.c", ORACLE_FILTER_C)?;

    // C: the `Z7_ST` build. `-D_7ZIP_ST` is the older spelling the SDK still
    // honours; passing both keeps this working either way.
    let common: &[&str] = &["-O2", "-DZ7_ST", "-D_7ZIP_ST"];
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());

    let build = |out: &Path, sources: &[PathBuf]| -> Result<(), String> {
        let mut cmd = Command::new(&cc);
        cmd.args(common).arg("-I").arg(&c).arg("-o").arg(out);
        cmd.args(sources);
        let st = cmd
            .status()
            .map_err(|e| format!("run {cc}: {e}; set CC to a C compiler"))?;
        if !st.success() {
            return Err(format!("{cc} failed building {}", out.display()));
        }
        Ok(())
    };

    let core = |names: &[&str]| -> Vec<PathBuf> { names.iter().map(|n| c.join(n)).collect() };

    let util = dest.join("lzma");
    let mut util_srcs = core(&[
        "Util/Lzma/LzmaUtil.c",
        "Alloc.c",
        "CpuArch.c",
        "LzFind.c",
        "LzmaDec.c",
        "LzmaEnc.c",
        "7zFile.c",
        "7zStream.c",
    ]);
    util_srcs.sort();
    build(&util, &util_srcs)?;

    let oracle = dest.join("lzma-oracle");
    let mut oracle_srcs = vec![oracle_src];
    oracle_srcs.extend(core(&["Alloc.c", "CpuArch.c", "LzFind.c", "LzmaEnc.c"]));
    build(&oracle, &oracle_srcs)?;

    let oracle2 = dest.join("lzma2-oracle");
    let mut oracle2_srcs = vec![oracle2_src];
    oracle2_srcs.extend(core(&[
        "Alloc.c",
        "CpuArch.c",
        "LzFind.c",
        "LzmaEnc.c",
        "Lzma2Enc.c",
    ]));
    build(&oracle2, &oracle2_srcs)?;

    let filters = dest.join("filter-oracle");
    let mut filter_srcs = vec![filter_src];
    filter_srcs.extend(core(&[
        "CpuArch.c",
        "Bra.c",
        "Bra86.c",
        "BraIA64.c",
        "Delta.c",
    ]));
    build(&filters, &filter_srcs)?;

    Ok(vec![util, oracle, oracle2, filters])
}

/// The SDK's own branch converters and delta filter, driven from the command
/// line, so the filter port can be compared byte for byte against them.
const ORACLE_FILTER_C: &str = r##"/* The SDK's BCJ and delta filters, for parity testing.
   Usage: filter-oracle <name> <enc|dec> <start-offset-or-distance> <in> <out>
   <name> is one of x86 ppc ia64 arm armt sparc arm64 riscv delta. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "Bra.h"
#include "Delta.h"

int main(int argc, char **argv)
{
  if (argc != 6) { fprintf(stderr, "usage: filter-oracle name enc|dec n in out\n"); return 2; }
  {
  const char *name = argv[1];
  const int enc = strcmp(argv[2], "enc") == 0;
  const UInt32 n = (UInt32)strtoul(argv[3], NULL, 0);
  FILE *fi = fopen(argv[4], "rb"), *fo;
  Byte *buf; size_t size;
  if (!fi) { perror("open in"); return 2; }
  fseek(fi, 0, SEEK_END); size = (size_t)ftell(fi); fseek(fi, 0, SEEK_SET);
  buf = (Byte *)malloc(size ? size : 1);
  if (!buf) return 2;
  if (size && fread(buf, 1, size, fi) != size) { perror("read"); return 2; }
  fclose(fi);

  if (strcmp(name, "delta") == 0)
  {
    Byte state[DELTA_STATE_SIZE];
    Delta_Init(state);
    if (enc) Delta_Encode(state, (unsigned)n, buf, size);
    else     Delta_Decode(state, (unsigned)n, buf, size);
  }
  else if (strcmp(name, "x86") == 0)
  {
    UInt32 state = Z7_BRANCH_CONV_ST_X86_STATE_INIT_VAL;
    if (enc) z7_BranchConvSt_X86_Enc(buf, size, n, &state);
    else     z7_BranchConvSt_X86_Dec(buf, size, n, &state);
  }
#define CONV(s, id) \
  else if (strcmp(name, s) == 0) \
  { if (enc) z7_BranchConv_ ## id ## _Enc(buf, size, n); \
    else     z7_BranchConv_ ## id ## _Dec(buf, size, n); }
  CONV("ppc",   PPC)
  CONV("ia64",  IA64)
  CONV("arm",   ARM)
  CONV("armt",  ARMT)
  CONV("sparc", SPARC)
  CONV("arm64", ARM64)
  CONV("riscv", RISCV)
  else { fprintf(stderr, "unknown filter %s\n", name); return 2; }

  fo = fopen(argv[5], "wb");
  if (!fo) { perror("open out"); return 2; }
  if (size && fwrite(buf, 1, size, fo) != size) { perror("write"); return 2; }
  fclose(fo);
  free(buf);
  return 0;
  }
}
"##;

/// A props-driven LZMA-Alone encoder over the pinned SDK. It is written out by
/// [`lzma_util`] rather than committed, so that nothing in this repository
/// carries a copy of the reference sources.
const ORACLE_C: &str = r#"/* Props-driven LZMA-Alone encoder over the pinned SDK, for parity testing.
   Usage: oracle <level> <btMode> <numHashBytes> <lc> <lp> <pb> <fb> <dictSize> <in> <out> */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "LzmaEnc.h"
#include "Alloc.h"

typedef struct { ISeqInStream vt; const Byte *p; size_t rem; } MemIn;
static SRes MemIn_Read(ISeqInStreamPtr pp, void *buf, size_t *size) {
  MemIn *s = Z7_CONTAINER_FROM_VTBL(pp, MemIn, vt);
  size_t n = *size; if (n > s->rem) n = s->rem;
  memcpy(buf, s->p, n); s->p += n; s->rem -= n; *size = n; return SZ_OK;
}
typedef struct { ISeqOutStream vt; FILE *f; } FileOut;
static size_t FileOut_Write(ISeqOutStreamPtr pp, const void *buf, size_t size) {
  FileOut *s = Z7_CONTAINER_FROM_VTBL(pp, FileOut, vt);
  return fwrite(buf, 1, size, s->f);
}

int main(int argc, char **argv) {
  if (argc != 11) { fprintf(stderr, "bad args\n"); return 2; }
  CLzmaEncProps props; LzmaEncProps_Init(&props);
  props.level = atoi(argv[1]);
  props.btMode = atoi(argv[2]);
  props.numHashBytes = atoi(argv[3]);
  props.lc = atoi(argv[4]); props.lp = atoi(argv[5]); props.pb = atoi(argv[6]);
  props.fb = atoi(argv[7]);
  props.dictSize = (UInt32)strtoul(argv[8], NULL, 10);

  FILE *fi = fopen(argv[9], "rb"); if (!fi) return 3;
  fseek(fi, 0, SEEK_END); long n = ftell(fi); fseek(fi, 0, SEEK_SET);
  Byte *src = (Byte *)malloc((size_t)n + 1);
  if (n && fread(src, 1, (size_t)n, fi) != (size_t)n) return 3;
  fclose(fi);

  FILE *fo = fopen(argv[10], "wb"); if (!fo) return 3;
  CLzmaEncHandle enc = LzmaEnc_Create(&g_Alloc);
  if (!enc) return 4;
  if (LzmaEnc_SetProps(enc, &props) != SZ_OK) { fprintf(stderr, "setprops\n"); return 5; }

  Byte header[LZMA_PROPS_SIZE + 8]; size_t hs = LZMA_PROPS_SIZE;
  if (LzmaEnc_WriteProperties(enc, header, &hs) != SZ_OK) return 6;
  for (int i = 0; i < 8; i++) header[hs++] = (Byte)((UInt64)n >> (8 * i));
  fwrite(header, 1, hs, fo);

  MemIn in; in.vt.Read = MemIn_Read; in.p = src; in.rem = (size_t)n;
  FileOut out; out.vt.Write = FileOut_Write; out.f = fo;
  SRes res = LzmaEnc_Encode(enc, &out.vt, &in.vt, NULL, &g_Alloc, &g_Alloc);
  LzmaEnc_Destroy(enc, &g_Alloc, &g_Alloc);
  fclose(fo);
  if (res != SZ_OK) { fprintf(stderr, "encode res=%d\n", res); return 7; }
  return 0;
}
"#;

/// The LZMA2 counterpart of [`ORACLE_C`], written out the same way.
const ORACLE2_C: &str = r#"/* Props-driven LZMA2 encoder over the pinned SDK, for parity testing. It
   writes the single LZMA2 property byte, then the raw LZMA2 stream.
   Usage: lzma2-oracle <level> <btMode> <numHashBytes> <lc> <lp> <pb> <fb> <dictSize> <in> <out> */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "Lzma2Enc.h"
#include "Alloc.h"

typedef struct { ISeqInStream vt; const Byte *p; size_t rem; } MemIn;
static SRes MemIn_Read(ISeqInStreamPtr pp, void *buf, size_t *size) {
  MemIn *s = Z7_CONTAINER_FROM_VTBL(pp, MemIn, vt);
  size_t n = *size; if (n > s->rem) n = s->rem;
  memcpy(buf, s->p, n); s->p += n; s->rem -= n; *size = n; return SZ_OK;
}
typedef struct { ISeqOutStream vt; FILE *f; } FileOut;
static size_t FileOut_Write(ISeqOutStreamPtr pp, const void *buf, size_t size) {
  FileOut *s = Z7_CONTAINER_FROM_VTBL(pp, FileOut, vt);
  return fwrite(buf, 1, size, s->f);
}

int main(int argc, char **argv) {
  if (argc != 11) { fprintf(stderr, "bad args\n"); return 2; }
  CLzma2EncProps props; Lzma2EncProps_Init(&props);
  props.lzmaProps.level = atoi(argv[1]);
  props.lzmaProps.btMode = atoi(argv[2]);
  props.lzmaProps.numHashBytes = atoi(argv[3]);
  props.lzmaProps.lc = atoi(argv[4]);
  props.lzmaProps.lp = atoi(argv[5]);
  props.lzmaProps.pb = atoi(argv[6]);
  props.lzmaProps.fb = atoi(argv[7]);
  props.lzmaProps.dictSize = (UInt32)strtoul(argv[8], NULL, 10);
  /* One solid block: this port has no block threads. */
  props.blockSize = LZMA2_ENC_PROPS_BLOCK_SIZE_SOLID;
  props.numBlockThreads_Max = 1;
  props.numBlockThreads_Reduced = 1;
  props.numTotalThreads = 1;
  props.lzmaProps.numThreads = 1;

  FILE *fi = fopen(argv[9], "rb"); if (!fi) return 3;
  fseek(fi, 0, SEEK_END); long n = ftell(fi); fseek(fi, 0, SEEK_SET);
  Byte *src = (Byte *)malloc((size_t)n + 1);
  if (n && fread(src, 1, (size_t)n, fi) != (size_t)n) return 3;
  fclose(fi);

  FILE *fo = fopen(argv[10], "wb"); if (!fo) return 3;
  CLzma2EncHandle enc = Lzma2Enc_Create(&g_Alloc, &g_Alloc);
  if (!enc) return 4;
  if (Lzma2Enc_SetProps(enc, &props) != SZ_OK) { fprintf(stderr, "setprops\n"); return 5; }
  Lzma2Enc_SetDataSize(enc, (UInt64)n);
  Byte prop = Lzma2Enc_WriteProperties(enc);
  fwrite(&prop, 1, 1, fo);

  MemIn in; in.vt.Read = MemIn_Read; in.p = src; in.rem = (size_t)n;
  FileOut out; out.vt.Write = FileOut_Write; out.f = fo;
  SRes res = Lzma2Enc_Encode2(enc, &out.vt, NULL, NULL, &in.vt, NULL, 0, NULL);
  Lzma2Enc_Destroy(enc);
  fclose(fo);
  if (res != SZ_OK) { fprintf(stderr, "encode res=%d\n", res); return 7; }
  return 0;
}
"#;

/// The SHA-256 of `data`, as lowercase hex, for comparing a download with the
/// digest pinned above.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(data);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}
