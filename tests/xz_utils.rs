//! XZ Utils' own decoder test files, through every reader this crate has.
//!
//! `tests/files` in XZ Utils is a suite of small `.xz` and `.lzma` streams,
//! many made by hand, each documented in its README as one that must decode,
//! must fail, or uses a feature the format reserves. They are not committed
//! here: `cargo xtask xz-tests` fetches the pinned release, checks every file
//! against `tests/xz-utils.manifest`, and copies them to
//! `target/xz-utils-tests` (or wherever `LZMA_TURBO_XZ_TESTS` points).
//! Without them this test says so and passes, unless
//! `LZMA_TURBO_XZ_TESTS_REQUIRE` is set, as CI sets it.
//!
//! The manifest also carries, for each good file, the SHA-256 of what `xz`
//! 5.8.3 decodes it to, so "decodes" means "to the same bytes as xz".

#![cfg(all(feature = "xz", any(feature = "crypto", feature = "native-crypto")))]

mod common;

use std::{
    io::{self, Cursor, Read},
    path::PathBuf,
};

use lzma_turbo::{
    DrainStatus, LzmaReader, XzAdaptiveDecoder, XzError, XzErrorKind, XzOptions, XzParallelReader,
    XzReader, crypto::Sha256,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Good,
    Bad,
    Unsupported,
    Unverifiable,
}

struct Case {
    name: String,
    file_sha: String,
    expect: Expect,
    output_sha: Option<String>,
    bytes: Vec<u8>,
}

fn hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The manifest's cases with their bytes, or `None` if the files have not
/// been fetched.
fn cases() -> Option<Vec<Case>> {
    let dir = std::env::var_os("LZMA_TURBO_XZ_TESTS").map_or_else(
        || common::repo_root().join("target").join("xz-utils-tests"),
        PathBuf::from,
    );
    if !dir.is_dir() {
        assert!(
            std::env::var_os("LZMA_TURBO_XZ_TESTS_REQUIRE").is_none(),
            "no XZ Utils test files at {}; run `cargo xtask xz-tests`",
            dir.display()
        );
        eprintln!(
            "skipping: no XZ Utils test files at {}; `cargo xtask xz-tests` fetches them",
            dir.display()
        );
        return None;
    }
    let manifest = std::fs::read_to_string(common::repo_root().join("tests/xz-utils.manifest"))
        .expect("read tests/xz-utils.manifest");
    let mut out = Vec::new();
    for line in manifest.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let [file_sha, expect, output_sha, name] = f[..] else {
            panic!("manifest line is not four fields: {line}");
        };
        let expect = match expect {
            "good" => Expect::Good,
            "bad" => Expect::Bad,
            "unsupported" => Expect::Unsupported,
            "unverifiable" => Expect::Unverifiable,
            other => panic!("unknown expectation {other}"),
        };
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("read {name}: {e}"));
        let case = Case {
            name: name.to_owned(),
            file_sha: file_sha.to_owned(),
            expect,
            output_sha: (output_sha != "-").then(|| output_sha.to_owned()),
            bytes,
        };
        // The fetch checked these too; a stale or hand-edited directory
        // should fail here rather than as a mysterious decode result.
        assert_eq!(
            hex(&case.bytes),
            case.file_sha,
            "{name} is not the file the manifest pins"
        );
        out.push(case);
    }
    assert!(
        out.len() > 60,
        "the manifest lists only {} files",
        out.len()
    );
    Some(out)
}

/// A reader that hands out one byte per call, so every state the container
/// reader can stop in is a state it does stop in.
struct Trickle<'a>(&'a [u8]);

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match (self.0.split_first(), buf.first_mut()) {
            (Some((&b, rest)), Some(slot)) => {
                *slot = b;
                self.0 = rest;
                Ok(1)
            }
            _ => Ok(0),
        }
    }
}

fn read_all(mut r: impl Read) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    r.read_to_end(&mut out)?;
    Ok(out)
}

fn xz_kind(e: &io::Error) -> Option<XzErrorKind> {
    e.get_ref()
        .and_then(|inner| inner.downcast_ref::<XzError>())
        .map(|x| x.kind)
}

fn adaptive(data: &[u8], threads: usize, chunk: usize) -> Result<Vec<u8>, XzError> {
    let mut dec = XzAdaptiveDecoder::new(XzOptions::default().with_threads(threads));
    let mut out = Vec::new();
    let mut pos = 0;
    if data.is_empty() {
        dec.end_of_input();
    }
    loop {
        if pos < data.len() {
            pos += dec.feed(&data[pos..(pos + chunk).min(data.len())])?;
            if pos == data.len() {
                dec.end_of_input();
            }
        }
        let status = dec.drain(|off, bytes| {
            let off = usize::try_from(off).expect("offset");
            if out.len() < off + bytes.len() {
                out.resize(off + bytes.len(), 0);
            }
            out[off..off + bytes.len()].copy_from_slice(bytes);
        })?;
        match status {
            DrainStatus::Finished => return Ok(out),
            DrainStatus::NeedsMoreInput if pos == data.len() => {
                panic!("the adaptive decoder wanted input after the file ended")
            }
            _ => {}
        }
    }
}

/// What one reader made of a file: its output, or the xz error kind when
/// the reader reports one.
type Outcome = Result<Vec<u8>, Option<XzErrorKind>>;

/// Every way this crate can decode a `.xz` file, by name, with its result.
fn every_xz_reader(data: &[u8]) -> Vec<(&'static str, Outcome)> {
    let io = |r: io::Result<Vec<u8>>| r.map_err(|e| xz_kind(&e));
    let parallel = |threads| match XzParallelReader::with_options(
        Cursor::new(data.to_vec()),
        XzOptions::default().with_threads(threads),
    ) {
        Ok(r) => io(read_all(r)),
        Err(e) => Err(Some(e.kind)),
    };
    vec![
        ("XzReader", io(read_all(XzReader::new(data)))),
        (
            "XzReader, a byte at a time",
            io(read_all(XzReader::new(Trickle(data)))),
        ),
        ("XzParallelReader, 1 thread", parallel(1)),
        ("XzParallelReader, 4 threads", parallel(4)),
        (
            "XzAdaptiveDecoder, 1 thread",
            adaptive(data, 1, usize::MAX).map_err(|e| Some(e.kind)),
        ),
        (
            "XzAdaptiveDecoder, 4 threads, 7-byte feeds",
            adaptive(data, 4, 7).map_err(|e| Some(e.kind)),
        ),
    ]
}

/// The `.lzma` readers. Only the reader: the raw decoder under it hands the
/// declared size and the end marker back to its caller as statuses, and
/// tools/sdk-oracle holds it to the LZMA SDK call by call on these files.
fn every_lzma_reader(data: &[u8]) -> Vec<(&'static str, Outcome)> {
    let io = |r: io::Result<Vec<u8>>| r.map_err(|_| None);
    vec![
        ("LzmaReader", io(LzmaReader::new(data).and_then(read_all))),
        (
            "LzmaReader, a byte at a time",
            io(LzmaReader::new(Trickle(data)).and_then(read_all)),
        ),
    ]
}

/// Where this crate does not yet give the README's verdict, by file and
/// reader. Each is a decoder bug to fix; the test fails on any divergence not
/// listed here, and on any listed one that no longer happens, so the list can
/// only shrink.
const KNOWN_DIVERGENCES: &[(&str, &str)] = &[
    // A stream with no blocks: the parallel reader's index check refuses it.
    ("good-0-empty.xz", "XzParallelReader, 1 thread"),
    ("good-0-empty.xz", "XzParallelReader, 4 threads"),
    ("good-0cat-empty.xz", "XzParallelReader, 1 thread"),
    ("good-0cat-empty.xz", "XzParallelReader, 4 threads"),
    ("good-0catpad-empty.xz", "XzParallelReader, 1 thread"),
    ("good-0catpad-empty.xz", "XzParallelReader, 4 threads"),
    ("good-0pad-empty.xz", "XzParallelReader, 1 thread"),
    ("good-0pad-empty.xz", "XzParallelReader, 4 threads"),
    // Three Delta filters before LZMA2: a repeated filter is refused as a
    // bad chain, which the format allows.
    ("good-1-3delta-lzma2.xz", "XzReader"),
    ("good-1-3delta-lzma2.xz", "XzReader, a byte at a time"),
    ("good-1-3delta-lzma2.xz", "XzParallelReader, 1 thread"),
    ("good-1-3delta-lzma2.xz", "XzParallelReader, 4 threads"),
    ("good-1-3delta-lzma2.xz", "XzAdaptiveDecoder, 1 thread"),
    (
        "good-1-3delta-lzma2.xz",
        "XzAdaptiveDecoder, 4 threads, 7-byte feeds",
    ),
    // LZMA2 that stops at the block's declared sizes without its end
    // marker: the parallel paths take the sizes as the end.
    ("bad-1-lzma2-11.xz", "XzParallelReader, 1 thread"),
    ("bad-1-lzma2-11.xz", "XzParallelReader, 4 threads"),
    (
        "bad-1-lzma2-11.xz",
        "XzAdaptiveDecoder, 4 threads, 7-byte feeds",
    ),
    // The end marker arrives before the header's declared size.
    ("bad-too_big_size-with_eopm.lzma", "LzmaReader"),
    (
        "bad-too_big_size-with_eopm.lzma",
        "LzmaReader, a byte at a time",
    ),
    // A literal where the end marker should be after the declared size:
    // caught when the reader holds the whole file, missed a byte at a time.
    (
        "bad-too_small_size-without_eopm-1.lzma",
        "LzmaReader, a byte at a time",
    ),
];

#[test]
fn every_xz_utils_test_file_gets_the_verdict_its_readme_gives() {
    let Some(cases) = cases() else { return };
    let mut failures = Vec::new();
    for case in &cases {
        let results = if case.name.ends_with(".lzma") {
            every_lzma_reader(&case.bytes)
        } else {
            every_xz_reader(&case.bytes)
        };
        for (reader, result) in results {
            let problem = match (case.expect, &result) {
                (Expect::Good, Ok(out)) => {
                    let want = case
                        .output_sha
                        .as_deref()
                        .expect("good files have an output");
                    (hex(out) != want)
                        .then(|| format!("decoded {} bytes that are not xz's", out.len()))
                }
                (Expect::Good, Err(kind)) => Some(format!("refused a good file: {kind:?}")),
                (Expect::Bad, Ok(out)) => {
                    Some(format!("accepted a bad file ({} bytes)", out.len()))
                }
                (Expect::Bad, Err(_)) => None,
                (Expect::Unsupported, Err(Some(kind))) if unsupported(*kind) => None,
                (Expect::Unsupported | Expect::Unverifiable, other) => match (case.expect, other) {
                    (Expect::Unverifiable, Err(Some(XzErrorKind::UnsupportedCheck))) => None,
                    _ => Some(format!("wanted it refused as unsupported, got {other:?}")),
                },
            };
            let known = KNOWN_DIVERGENCES.contains(&(case.name.as_str(), reader));
            match (problem, known) {
                (Some(p), false) => failures.push(format!("{} via {reader}: {p}", case.name)),
                (None, true) => failures.push(format!(
                    "{} via {reader} now gets its verdict: take it off KNOWN_DIVERGENCES",
                    case.name
                )),
                _ => {}
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// What the format spec reserves for later: an unknown filter, or a
/// reserved flag or padding byte in a block header.
fn unsupported(kind: XzErrorKind) -> bool {
    matches!(
        kind,
        XzErrorKind::UnsupportedFilter
            | XzErrorKind::BadFilterChain
            | XzErrorKind::BadBlockHeader
            | XzErrorKind::BadPadding
    )
}

#[test]
fn an_unknown_check_decodes_when_the_caller_allows_it() {
    let Some(cases) = cases() else { return };
    for case in cases.iter().filter(|c| c.expect == Expect::Unverifiable) {
        let out = read_all(XzReader::new(&case.bytes[..]).allow_unverifiable(true))
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(Some(hex(&out)), case.output_sha, "{}", case.name);
    }
}
