//! One input at one setting is one stream, on every platform.
//!
//! The parity tests are per platform: each runner builds the SDK's encoder
//! from the pinned sources and compares this crate against the binary it just
//! built. That proves this port agrees with the C *on that machine*, and it
//! would go on passing if both drifted together - if a compiler folded the
//! same constant differently, or if a `usize` reached a decision it should not
//! have. `docs/encoder.md` lists two places the C's own output depends on
//! `sizeof(size_t)` and this port pins them to their 64-bit values; nothing
//! but this file would notice if a third appeared.
//!
//! So a handful of cases have their SHA-256 written down, once, and every
//! platform in the `test` job checks the bytes it produces against it. The
//! digest is `crate::crypto`'s, the same SHA-256 the `.xz` reader verifies
//! check type 10 with; nothing here hashes by hand.
//!
//! `cargo xtask golden --write` regenerates `tests/golden.manifest` and
//! `cargo xtask golden --check` is this test, which is what CI runs.

#![cfg(all(
    feature = "enc",
    feature = "xz",
    any(feature = "crypto", feature = "native-crypto")
))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lzma_turbo::xz::bcj::BcjKind;
use lzma_turbo::xz::{CheckType, FILTER_DELTA, FilterFlags};
use lzma_turbo::{
    Lzma2Encoder, LzmaEncProps, MatchFinderKind, encode_lzma_alone, encode_lzma2,
    encode_xz_with_filters,
};

mod corpus;

/// The cases, by name. Nothing here is taken from the input or the
/// environment: a golden vector whose settings could move is not one.
const CASES: &[&str] = &[
    "random-tiny",
    "repeats-small",
    "zeros-65536",
    "text-chunk-under",
    "text-big",
];

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden.manifest")
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The crate's own SHA-256, never a hand-written one.
fn digest(bytes: &[u8]) -> String {
    let mut h = lzma_turbo::crypto::Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

/// Every golden stream: `name` to the SHA-256 of the bytes this build makes.
fn produce() -> BTreeMap<String, String> {
    assert_eq!(
        corpus::max_len(),
        usize::MAX,
        "LZMA_TURBO_CORPUS_MAX is set: these digests are of the whole corpus, \
         and a truncated one would compare a different stream"
    );
    let corpus: BTreeMap<String, Vec<u8>> = corpus::corpus().into_iter().collect();
    let mut out = BTreeMap::new();
    for case in CASES {
        let src = corpus
            .get(*case)
            .unwrap_or_else(|| panic!("the corpus has no case {case}"));

        // Single-threaded LZMA1, the optimal parser and a binary tree.
        let props = LzmaEncProps::new()
            .with_level(6)
            .with_match_finder(MatchFinderKind::Bt4)
            .with_dict_size(1 << 20);
        out.insert(
            format!("lzma1-l6-bt4/{case}"),
            digest(&encode_lzma_alone(src, &props).expect("encode")),
        );

        // Single-threaded raw LZMA2, the fast parser and a hash chain.
        let props = LzmaEncProps::new()
            .with_level(1)
            .with_match_finder(MatchFinderKind::Hc4)
            .with_dict_size(1 << 16);
        let (prop, bytes) = encode_lzma2(src, &props).expect("encode");
        out.insert(format!("lzma2-l1-hc4/{case}"), digest(&bytes));
        out.insert(format!("lzma2-l1-hc4-prop/{case}"), digest(&[prop]));

        // Block-parallel LZMA2 at a fixed block size. The thread count must
        // not reach the bytes, so all three are compared with one digest
        // rather than three.
        let props = LzmaEncProps::new().with_level(5).with_dict_size(1 << 20);
        let mut mt: Option<String> = None;
        for threads in [1usize, 2, 4] {
            let mut enc = Lzma2Encoder::new(&props).expect("encoder");
            enc.set_block_size(1 << 16);
            enc.set_threads(threads);
            let bytes = enc.encode_to_vec(src).expect("encode");
            let d = digest(&bytes);
            match &mt {
                None => mt = Some(d),
                Some(first) => assert_eq!(
                    &d, first,
                    "{case}: {threads} block threads changed the bytes"
                ),
            }
        }
        out.insert(
            format!("lzma2-l5-block64k/{case}"),
            mt.expect("at least one thread count"),
        );

        // The threaded match finder, which unlike the block count *does*
        // change the bytes - deliberately, because the C's own two builds
        // differ there. It has to be the same stream everywhere all the same.
        let props = LzmaEncProps::new()
            .with_level(9)
            .with_match_finder(MatchFinderKind::Bt4)
            .with_dict_size(1 << 20)
            .with_num_threads(2);
        let mut enc = Lzma2Encoder::new(&props).expect("encoder");
        enc.set_data_size(src.len() as u64);
        out.insert(
            format!("lzma2-l9-mf2/{case}"),
            digest(&enc.encode_to_vec(src).expect("encode")),
        );

        // One filter chain through the .xz writer: delta, then the x86 branch
        // converter, then LZMA2, at a fixed block size and check.
        let props = LzmaEncProps::new().with_level(5).with_dict_size(1 << 20);
        let filters = [
            FilterFlags::new(FILTER_DELTA, &[3]).expect("delta"),
            FilterFlags::new(BcjKind::X86.filter_id(), &[]).expect("x86"),
        ];
        out.insert(
            format!("xz-delta4-x86-l5-block64k/{case}"),
            digest(
                &encode_xz_with_filters(src, &props, CheckType::Crc64, 1 << 16, &filters)
                    .expect("encode"),
            ),
        );
    }
    out
}

fn render(digests: &BTreeMap<String, String>) -> String {
    let mut text = String::from(
        "# The SHA-256 of one stream per (input, setting), the same on every\n\
         # platform. `cargo xtask golden --write` writes this file;\n\
         # `cargo xtask golden --check` and CI's `test` job compare against it.\n\
         # Marked binary in .gitattributes, as every committed vector is.\n",
    );
    for (name, digest) in digests {
        text.push_str(&format!("{digest}  {name}\n"));
    }
    text
}

fn parse(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (digest, name) = line
            .split_once("  ")
            .unwrap_or_else(|| panic!("manifest line is not `<digest>  <name>`: {line}"));
        out.insert(name.to_owned(), digest.to_owned());
    }
    out
}

#[test]
fn the_encoder_produces_the_same_bytes_on_every_platform() {
    let produced = produce();
    let path = manifest_path();

    if std::env::var_os("LZMA_TURBO_GOLDEN_WRITE").is_some() {
        std::fs::write(&path, render(&produced)).expect("write the manifest");
        eprintln!("wrote {} digests to {}", produced.len(), path.display());
        return;
    }

    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e}; run `cargo xtask golden --write`",
            path.display()
        )
    });
    let want = parse(&text);
    let mut wrong: Vec<String> = Vec::new();
    for (name, digest) in &produced {
        match want.get(name) {
            Some(expected) if expected == digest => {}
            Some(expected) => wrong.push(format!("{name}: {digest}, the manifest says {expected}")),
            None => wrong.push(format!("{name}: {digest}, not in the manifest")),
        }
    }
    for name in want.keys() {
        if !produced.contains_key(name) {
            wrong.push(format!("{name}: in the manifest, not produced"));
        }
    }
    assert!(
        wrong.is_empty(),
        "the encoder's bytes are not the committed ones:\n{}\n\
         If this is an intended change, `cargo xtask golden --write` and say why \
         in the changelog.",
        wrong.join("\n")
    );
}
