//! `cargo xtask asm-lab build|verify`: the harness of `asm-lab/`, the arm64
//! decode-loop experiments written up in `asm-lab/RESULTS.md`.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use crate::{
    cmd::{repo_root, run_to_file_quietly},
    rng::Rng,
};

pub const USAGE: &str = "\
cargo xtask asm-lab build <c | LzmaDecOpt-stock.S | variants/NN-name.S>
    Build the LZMA SDK's `7lzma` CLI against a chosen arm64 decode loop, into
    asm-lab/build/7lzma-<tag>. `c` is the C fast loop with no assembly.
    SDK=<checkout of github.com/ip7z/7zip> is required; CC defaults to clang;
    PROB32=1 widens CLzmaProb to 32 bits on both the C and the asm side.

cargo xtask asm-lab verify <decoder-binary>
    The correctness gate for a variant: decodes the two project fixtures and
    a spread of small `xz --format=lzma` streams and compares every byte.
    LZLAB_FIXTURES and LZLAB_WORK override where fixtures and scratch live.";

pub fn asm_lab(mut args: impl Iterator<Item = String>) -> ExitCode {
    let ok = match (args.next().as_deref(), args.next(), args.next()) {
        (Some("build"), Some(asm), None) => build(&asm),
        (Some("verify"), Some(binary), None) => verify(Path::new(&binary)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Flags mirror what 7-Zip's own makefile uses for a release build
/// (`C/7zip_gcc_c.mak`); `C/Util/Lzma/makefile.gcc` adds `-DZ7_LZMA_DEC_OPT`
/// to `LzmaDec.c` and assembles `Asm/arm64/LzmaDecOpt.S` with the same flags.
fn build(asm: &str) -> bool {
    let lab = repo_root().join("asm-lab");
    let Some(sdk) = env::var_os("SDK") else {
        eprintln!("asm-lab: set SDK to a checkout of github.com/ip7z/7zip");
        return false;
    };
    let c_dir = PathBuf::from(sdk).join("C");
    let cc = env::var("CC").unwrap_or_else(|_| "clang".into());
    let prob32 = env::var("PROB32").is_ok_and(|v| v == "1");

    let mut cflags = vec![
        "-O2",
        "-c",
        "-DNDEBUG",
        "-D_REENTRANT",
        "-D_FILE_OFFSET_BITS=64",
        "-D_LARGEFILE_SOURCE",
    ];
    let mut asmflags = vec!["-DLAB_NOFLAG"];
    if prob32 {
        cflags.push("-DZ7_LZMA_PROB32");
        asmflags.push("-D_LZMA_PROB32");
    }

    let is_c = asm == "c";
    let mut tag = if is_c {
        "c".to_owned()
    } else {
        if !lab.join(asm).is_file() {
            eprintln!("asm-lab: no such asm: {}", lab.join(asm).display());
            return false;
        }
        let stem = Path::new(asm).file_stem().expect("a file name");
        let stem = stem.to_string_lossy();
        stem.strip_prefix("LzmaDecOpt-").unwrap_or(&stem).to_owned()
    };
    if prob32 {
        tag.push_str("-p32");
    }

    let out = lab.join("build");
    let objects = out.join(&tag);
    fs::create_dir_all(&objects).expect("create the build directory");

    let compile = |flags: &[&str], extra: &[&str], source: &Path, object: &Path| {
        Command::new(&cc)
            .args(flags)
            .args(extra)
            .arg("-o")
            .arg(object)
            .arg(source)
            .status()
            .is_ok_and(|status| status.success())
    };

    let mut linked = Vec::new();
    for name in [
        "7zFile",
        "7zStream",
        "Alloc",
        "CpuArch",
        "LzFind",
        "LzFindMt",
        "LzFindOpt",
        "LzmaEnc",
        "Threads",
    ] {
        let source = c_dir.join(format!("{name}.c"));
        let object = objects.join(format!("{name}.o"));
        if !newer(&object, &source) && !compile(&cflags, &[], &source, &object) {
            return false;
        }
        linked.push(object);
    }
    let util = objects.join("LzmaUtil.o");
    if !compile(&cflags, &[], &c_dir.join("Util/Lzma/LzmaUtil.c"), &util) {
        return false;
    }
    linked.push(util);

    let dec = objects.join("LzmaDec.o");
    let dec_flags: &[&str] = if is_c { &[] } else { &["-DZ7_LZMA_DEC_OPT"] };
    if !compile(&cflags, dec_flags, &c_dir.join("LzmaDec.c"), &dec) {
        return false;
    }
    linked.push(dec);
    if !is_c {
        // -I<lab> so the variant's `#include "7zAsm.S"` picks up the lab's copy.
        let include = format!("-I{}", lab.display());
        let mut extra = asmflags.clone();
        extra.push(&include);
        let opt = objects.join("LzmaDecOpt.o");
        if !compile(&cflags, &extra, &lab.join(asm), &opt) {
            return false;
        }
        linked.push(opt);
    }

    let binary = out.join(format!("7lzma-{tag}"));
    let ok = Command::new(&cc)
        .arg("-o")
        .arg(&binary)
        .args(&linked)
        .status()
        .is_ok_and(|status| status.success());
    if ok {
        println!("built {}", binary.display());
    }
    ok
}

fn newer(object: &Path, source: &Path) -> bool {
    let modified = |path: &Path| fs::metadata(path).and_then(|m| m.modified());
    match (modified(object), modified(source)) {
        (Ok(object), Ok(source)) => object > source,
        _ => false,
    }
}

/// The lc/lp/pb spread matters because lc+lp drive `lc2_lpMask` (which does
/// double duty as a shift amount) and pb drives `pbMask`, so a loop that only
/// ever sees the default lc=3,lp=0,pb=2 can be wrong and look right.
const OPTIONS: [&str; 9] = [
    "preset=0",
    "preset=6",
    "preset=9e",
    "lc=0,lp=0,pb=0",
    "lc=0,lp=2,pb=0",
    "lc=1,lp=1,pb=1",
    "lc=4,lp=0,pb=3",
    "lc=8,lp=0,pb=0",
    "lc=3,lp=0,pb=2,dict=1MiB",
];

fn verify(binary: &Path) -> bool {
    let root = repo_root();
    // The fixtures are generated once and gitignored, so they live in the
    // main checkout even when this runs from a worktree. The common git
    // directory is shared by both, and its parent is the main checkout.
    let fixtures = env::var_os("LZLAB_FIXTURES").map_or_else(
        || {
            let git = Command::new("git")
                .current_dir(&root)
                .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
                .stderr(Stdio::null())
                .output()
                .ok()
                .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim_end()));
            git.as_deref()
                .and_then(Path::parent)
                .unwrap_or(&root)
                .join("bench")
                .join("fixtures")
        },
        PathBuf::from,
    );
    let work = env::var_os("LZLAB_WORK")
        .map_or_else(env::temp_dir, PathBuf::from)
        .join("lzverify");
    fs::create_dir_all(&work).expect("create the scratch directory");

    write_corpora(&root, &work);

    let scratch = work.join("out.bin");
    let mut failed = false;
    let mut check = |name: &str, plain: &Path, packed: &Path| {
        let decoded = Command::new(binary)
            .arg("d")
            .arg(packed)
            .arg(&scratch)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        let same =
            decoded && matches!((fs::read(plain), fs::read(&scratch)), (Ok(a), Ok(b)) if a == b);
        println!("  {}  {name}", if same { "ok  " } else { "FAIL" });
        failed |= !same;
    };

    println!("verify {}", binary.display());
    for corpus in ["text", "rand", "rep", "src", "runs", "tiny", "empty"] {
        let plain = work.join(format!("{corpus}.bin"));
        for option in OPTIONS {
            let slug: String = option.chars().filter(|c| !",=".contains(*c)).collect();
            let packed = work.join(format!("{corpus}.{slug}.lzma"));
            if !packed.is_file() {
                let lzma1 = format!("--lzma1={option}");
                let plain = plain.to_string_lossy();
                // xz refuses the combinations the format does not allow.
                run_to_file_quietly("xz", &["--format=lzma", &lzma1, "-c", &plain], &packed);
            }
            if fs::metadata(&packed).is_ok_and(|m| m.len() > 0) {
                check(&format!("{corpus} [{option}]"), &plain, &packed);
            }
        }
    }
    for name in ["p256", "payload"] {
        let packed = fixtures.join(format!("{name}.bin.lzma"));
        if packed.is_file() {
            check(
                &format!("fixture {name}"),
                &fixtures.join(format!("{name}.bin")),
                &packed,
            );
        }
    }
    println!("{}", if failed { "FAILURES" } else { "ALL OK" });
    !failed
}

/// Different byte statistics stress different decoder paths.
fn write_corpora(root: &Path, work: &Path) {
    if work.join("text.bin").is_file() {
        return;
    }
    let mut rng = Rng::new(11);

    // Literal-heavy: lowercase words from a small vocabulary, one per line.
    let vocabulary: Vec<Vec<u8>> = (0..20_000)
        .map(|_| {
            (0..rng.range(2, 12))
                .map(|_| b'a' + rng.range(0, 25) as u8)
                .collect()
        })
        .collect();
    let mut text = Vec::with_capacity(3_000_016);
    while text.len() < 3_000_000 {
        text.extend_from_slice(&vocabulary[rng.range(0, vocabulary.len() - 1)]);
        text.push(b'\n');
    }
    text.truncate(3_000_000);

    let mut random = Vec::new();
    rng.fill(&mut random, 2_000_000); // incompressible
    let mut tiny = Vec::new();
    rng.fill(&mut tiny, 100);

    // Mixed literals and matches: this repository's own tree.
    let mut source = capture_bytes(root, &["archive", "HEAD"]);
    source.truncate(100_000_000);

    let write = |name: &str, bytes: &[u8]| {
        fs::write(work.join(name), bytes).expect("write a corpus file");
    };
    write("rand.bin", &random);
    write("rep.bin", &[&text[..], &text[..]].concat()); // one huge match
    write("src.bin", &source);
    write("runs.bin", &vec![b'a'; 100_000]); // rep0 short matches
    write("tiny.bin", &tiny);
    write("empty.bin", &[]);
    // Last, because its presence is what marks the corpora as written.
    write("text.bin", &text);
}

fn capture_bytes(root: &Path, args: &[&str]) -> Vec<u8> {
    Command::new("git")
        .current_dir(root)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map(|output| output.stdout)
        .unwrap_or_default()
}
