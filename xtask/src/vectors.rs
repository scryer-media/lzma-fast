//! `cargo xtask vectors [--check]`: every committed test vector, from code.
//!
//! The streams under `tests/data` and the seeds under `fuzz/corpus` are bytes
//! no reviewer can read. This task is where each one comes from: the sources
//! are generated here from fixed seeds, every stream is what one pinned `xz`
//! makes of one source with the options written below, and the one input no
//! encoder would produce, a fuzzer's reproducer, is spelled out in hex. With
//! `--check` it builds everything into a scratch directory and fails unless
//! the committed files are exactly those bytes and nothing else, which is
//! what CI runs: a changed or added blob that this file does not explain
//! does not get in.
//!
//! Needs `xz` 5.8.3 on `PATH` (`.github/scripts/install-xz.sh` installs it);
//! another release may encode differently, so any other is refused.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
};

use crate::{cmd::repo_root, rng::Rng};

const XZ_VERSION: &str = "xz (XZ Utils) 5.8.3";

/// LZMA1 (`.lzma`) streams of every source.
const LZMA: &[(&str, &[&str])] = &[
    ("p1", &["--format=lzma", "-1"]),
    ("p9e", &["--format=lzma", "-9e"]),
    // lc + lp at their LZMA1 maximum split, and a literal context of zero.
    (
        "lc0lp2pb0",
        &["--format=lzma", "--lzma1=dict=64KiB,lc=0,lp=2,pb=0"],
    ),
    ("lc4pb1", &["--format=lzma", "--lzma1=dict=1MiB,lc=4,pb=1"]),
];

/// Single-block `.xz` streams of every source. `-T1` is xz's single-threaded
/// encoder, which leaves the sizes out of the block header.
const XZ: &[(&str, &[&str])] = &[
    ("p1", &["-T1", "-1"]),
    ("lc1lp1pb0", &["-T1", "--lzma2=dict=64KiB,lc=1,lp=1,pb=0"]),
];

/// Container variations over `mixed`. `-T+1` is the multi-threaded encoder
/// on one thread, which records both sizes in each block header.
const MIXED_XZ: &[(&str, &[&str])] = &[
    ("mb", &["-T+1", "-1", "--block-size=16KiB"]),
    ("bcjx86", &["-T+1", "--x86", "--lzma2=preset=1"]),
    ("delta4", &["-T+1", "--delta=dist=4", "--lzma2=preset=1"]),
    ("sha256", &["-T+1", "-1", "--check=sha256"]),
    ("crc32", &["-T+1", "-1", "--check=crc32"]),
    ("nocheck", &["-T+1", "-1", "--check=none"]),
];

/// `spin.declared-size.xz`: a block header that declares a compressed size
/// its LZMA2 never finishes inside, the input a fuzzer found that once made
/// the container reader loop offering the decoder bytes it could not use
/// (`tests/xz_container.rs`). Not something an encoder writes, so it is
/// spelled out.
const SPIN: &str = "\
fd377a585a000004e6d6b44604c15ba08d0604002101100000000000bea661f1\
e1869faf535d00006ffdffffa3b7ff473e481572396151b89228e6a38607f9ee\
e41e82d32fc53a3c014bb17ec98a8a4d2fa30dd97fa6e38c231153e05918c575\
8afafafafafafafafafafafafafafafafafafafafafafafafafafafafafafafa\
fafafafafafafafafafafafafae277f8b6947f0c6ac0de744964e2e95c53b204\
d6b1f597000000008fc694a218e370f1000177a08d0600006664ebbeb1c467fb\
020000000004595a";

pub fn vectors(mut args: impl Iterator<Item = String>) -> ExitCode {
    let check = match args.next().as_deref() {
        None => false,
        Some("--check") => true,
        Some(other) => {
            eprintln!("vectors: unknown argument '{other}'; the only one is --check");
            return ExitCode::from(2);
        }
    };
    match Command::new("xz").arg("--version").output() {
        Ok(out) if String::from_utf8_lossy(&out.stdout).starts_with(XZ_VERSION) => {}
        Ok(out) => {
            eprintln!(
                "vectors: needs {XZ_VERSION}, found {}",
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .unwrap_or("?")
            );
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("vectors: cannot run xz: {e}");
            return ExitCode::FAILURE;
        }
    }

    let root = repo_root();
    let scratch = root.join("target").join("vectors");
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).expect("create target/vectors");
    let built = match build(&scratch) {
        Ok(built) => built,
        Err(e) => {
            eprintln!("vectors: {e}");
            return ExitCode::FAILURE;
        }
    };

    if check {
        compare(&root, &built)
    } else {
        for (path, bytes) in &built {
            let to = root.join(path);
            fs::create_dir_all(to.parent().expect("parent")).expect("create directory");
            fs::write(&to, bytes).unwrap_or_else(|e| panic!("write {path}: {e}"));
        }
        println!("vectors: wrote {} files", built.len());
        ExitCode::SUCCESS
    }
}

/// Every generated file, by its path from the repository root.
fn build(scratch: &Path) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    let sources = sources();
    for (stem, bytes) in &sources {
        let src = scratch.join(format!("src_{stem}.bin"));
        fs::write(&src, bytes).map_err(|e| e.to_string())?;
        out.insert(format!("tests/data/src_{stem}.bin"), bytes.clone());
        for (variant, options) in LZMA {
            out.insert(
                format!("tests/data/{stem}.{variant}.lzma"),
                xz(options, &src)?,
            );
        }
        for (variant, options) in XZ {
            out.insert(
                format!("tests/data/{stem}.{variant}.xz"),
                xz(options, &src)?,
            );
        }
    }

    let mixed = scratch.join("src_mixed.bin");
    for (variant, options) in MIXED_XZ {
        out.insert(
            format!("tests/data/mixed.{variant}.xz"),
            xz(options, &mixed)?,
        );
    }

    // Two streams with four bytes of stream padding between them.
    let tiny = xz(&["-T+1", "-1"], &scratch.join("src_tiny.bin"))?;
    let mut concat = tiny.clone();
    concat.extend_from_slice(&[0; 4]);
    concat.extend_from_slice(&tiny);
    out.insert("tests/data/tiny.concat.xz".into(), concat);

    out.insert("tests/data/spin.declared-size.xz".into(), hex(SPIN));

    // Fuzz seeds: a byte that picks the target's streaming shape (3: whole
    // input at once), then a stream.
    let seed = |stream: &[u8], extra: &[u8]| {
        let mut v = vec![3u8];
        v.extend_from_slice(extra);
        v.extend_from_slice(stream);
        v
    };
    out.insert(
        "fuzz/corpus/decode_lzma/mixed.p1.lzma.seed".into(),
        seed(&out["tests/data/mixed.p1.lzma"], &[]),
    );
    out.insert(
        "fuzz/corpus/decode_lzma/tiny.p9e.lzma.seed".into(),
        seed(&out["tests/data/tiny.p9e.lzma"], &[]),
    );
    // decode_lzma2 takes a dictionary property byte, then a raw LZMA2 stream:
    // the block of mixed.p1.xz.
    let (prop, block) = lzma2_block(&out["tests/data/mixed.p1.xz"])?;
    out.insert(
        "fuzz/corpus/decode_lzma2/mixed.lzma2.seed".into(),
        seed(&block, &[prop]),
    );
    for (name, options) in [
        (
            "bcj-sha256",
            &["-T+1", "--x86", "--lzma2=preset=1", "--check=sha256"][..],
        ),
        ("mixed.p6", &["-T+1", "-6"][..]),
        ("multiblock", &["-T+1", "-1", "--block-size=8KiB"][..]),
    ] {
        out.insert(
            format!("fuzz/corpus/decode_xz/{name}.xz.seed"),
            seed(&xz(options, &mixed)?, &[]),
        );
    }
    Ok(out)
}

/// The sources every stream is made from, by stem.
fn sources() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("text", text(&mut Rng::new(0x7E47), 98_304)),
        ("rand", {
            let mut v = Vec::new();
            Rng::new(0x4A4D).fill(&mut v, 24_576);
            v
        }),
        ("zeros", vec![0; 49_152]),
        ("mixed", mixed()),
        ("tiny", b"hello hello hello world\n".repeat(3)),
        ("empty", Vec::new()),
    ]
}

/// Lines of words drawn from a vocabulary of hex strings, most often from
/// its head: repetitive the way prose is, with long and short matches and a
/// literal alphabet of seventeen characters.
fn text(rng: &mut Rng, len: usize) -> Vec<u8> {
    let vocabulary: Vec<Vec<u8>> = (0..600)
        .map(|_| {
            let mut raw = Vec::new();
            let n = rng.range(1, 7);
            rng.fill(&mut raw, n);
            raw.iter()
                .flat_map(|b| format!("{b:02x}").into_bytes())
                .collect()
        })
        .collect();
    let mut out = Vec::with_capacity(len + 64);
    while out.len() < len {
        // The minimum of two draws favours the low indices.
        let i = rng.range(0, 599).min(rng.range(0, 599));
        out.extend_from_slice(&vocabulary[i]);
        out.push(if rng.chance(8) { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

/// 54 KiB that sends the decoder down every symbol kind: text, a run of one
/// byte, incompressible noise, x86-shaped call instructions for the BCJ
/// filter, a stretch built to be coded as repeat matches at each of the four
/// remembered distances, and a long copy of the start for a far match.
fn mixed() -> Vec<u8> {
    let mut rng = Rng::new(0x31CE);
    let mut out = text(&mut rng, 12 * 1024);
    out.resize(out.len() + 4 * 1024, 0xAA);
    rng.fill(&mut out, 8 * 1024);

    // Copies from four distances in turn, a literal or two between them, so
    // that the encoder's cheapest choice is rep0, rep1, rep2 and rep3.
    let distances = [3usize, 17, 90, 411];
    let end = out.len() + 12 * 1024;
    while out.len() < end {
        let from = out.len() - distances[rng.range(0, 3)];
        for k in 0..rng.range(2, 9) {
            out.push(out[from + k]);
        }
        for _ in 0..rng.range(1, 2) {
            out.push(b'a' + (rng.next() % 26) as u8);
        }
    }
    out.truncate(end);

    // `call rel32` every few bytes among small opcodes.
    let end = out.len() + 4 * 1024;
    while out.len() < end {
        out.push(0xE8);
        out.extend_from_slice(&(rng.range(0, 1 << 16) as u32).to_le_bytes());
        for _ in 0..rng.range(0, 6) {
            out.push([0x55, 0x89, 0xE5, 0x5D, 0xC3, 0x90][rng.range(0, 5)]);
        }
    }
    out.truncate(end);

    let far = out[..6 * 1024].to_vec();
    out.extend_from_slice(&far);
    let tail = 55_296 - out.len();
    out.extend((0..tail).map(|_| b"etaoin shrdlu"[rng.range(0, 12)]));
    out
}

fn xz(options: &[&str], input: &Path) -> Result<Vec<u8>, String> {
    let output = Command::new("xz")
        .args(options)
        .arg("-c")
        .arg(input)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("run xz: {e}"))?;
    if !output.status.success() {
        return Err(format!("xz {options:?} {} failed", input.display()));
    }
    Ok(output.stdout)
}

/// The LZMA2 filter's dictionary property and the raw LZMA2 stream, end
/// marker included, of a one-block `.xz`.
fn lzma2_block(xz: &[u8]) -> Result<(u8, Vec<u8>), String> {
    let header = 12;
    let size = (usize::from(xz[header]) + 1) * 4;
    // Block flags, the two size fields, then the filter: id 0x21, one
    // property byte.
    let filter = &xz[header..header + size];
    let at = filter
        .windows(2)
        .position(|w| w == [0x21, 0x01])
        .ok_or("no LZMA2 filter in the block header")?;
    let prop = filter[at + 2];
    let data = &xz[header + size..];
    let mut pos = 0;
    loop {
        let control = *data.get(pos).ok_or("LZMA2 stream runs off the block")?;
        if control == 0 {
            return Ok((prop, data[..=pos].to_vec()));
        }
        pos += if control < 0x80 {
            3 + usize::from(u16::from_be_bytes([data[pos + 1], data[pos + 2]])) + 1
        } else {
            let packed = usize::from(u16::from_be_bytes([data[pos + 3], data[pos + 4]])) + 1;
            5 + usize::from(control >= 0xC0) + packed
        };
    }
}

fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ascii"), 16).expect("hex"))
        .collect()
}

/// Fails on any committed file that differs from its generated bytes, any
/// generated file that is not committed, and any committed vector or seed
/// this task does not make.
fn compare(root: &Path, built: &BTreeMap<String, Vec<u8>>) -> ExitCode {
    let mut problems = Vec::new();
    for (path, bytes) in built {
        match fs::read(root.join(path)) {
            Ok(committed) if &committed == bytes => {}
            Ok(_) => problems.push(format!(
                "{path}: committed bytes differ from the generated ones"
            )),
            Err(_) => problems.push(format!("{path}: generated but not committed")),
        }
    }
    for dir in ["tests/data", "fuzz/corpus"] {
        for path in files(&root.join(dir)) {
            let name = path
                .strip_prefix(root)
                .expect("under the root")
                .to_string_lossy()
                .replace('\\', "/");
            let generated_kind = name.starts_with("tests/data/") || name.ends_with(".seed");
            if generated_kind && !built.contains_key(&name) && !name.ends_with(".gitignore") {
                problems.push(format!("{name}: committed, but nothing here generates it"));
            }
        }
    }
    if problems.is_empty() {
        println!(
            "vectors: all {} committed vectors are what this task generates",
            built.len()
        );
        ExitCode::SUCCESS
    } else {
        for p in &problems {
            eprintln!("vectors: {p}");
        }
        eprintln!(
            "vectors: run `cargo xtask vectors` to regenerate, and explain the change in the commit"
        );
        ExitCode::FAILURE
    }
}

fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files(&path));
        } else {
            out.push(path);
        }
    }
    out
}
