//! The `.xz` container, against `xz(1)` and against committed vectors.
//!
//! Every case here is differential: the bytes come out of `xz` (or out of a
//! vector `xz` produced), and what this crate decodes has to equal what went
//! in. The cases that need the `xz` binary skip themselves when it is absent,
//! so the suite still runs on a machine without it; the committed vectors do
//! not need it and cover the common shapes.
//!
//! The container lives behind the `xz` feature, so this whole file does with
//! it.
#![cfg(feature = "xz")]

mod common;

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use lzma_fast::xz::{XzOptions, XzReader};

/// Compresses `data` with the `xz` binary, or `None` if it is not installed.
///
/// The input goes through a temporary file rather than a pipe: `xz` writes its
/// output as it reads, so feeding a large payload down a pipe while nothing
/// drains stdout deadlocks both processes.
fn xz_compress(args: &[&str], data: &[u8]) -> Option<Vec<u8>> {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!(
        "lzma-fast-xz-{}-{}.bin",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, data).expect("temp input");
    let out = Command::new("xz")
        .args(args)
        .arg("-c")
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok();
    let _ = std::fs::remove_file(&path);
    let out = out?;
    assert!(out.status.success(), "xz {args:?} failed");
    Some(out.stdout)
}

/// Decodes with the default options, in one go.
fn decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    XzReader::new(data).read_to_end(&mut out)?;
    Ok(out)
}

/// Decodes in `chunk`-sized reads, which is what exercises the state machine's
/// ability to stop anywhere.
fn decode_chunked(data: &[u8], chunk: usize) -> std::io::Result<Vec<u8>> {
    let mut r = XzReader::new(data);
    let mut out = Vec::new();
    let mut buf = vec![0u8; chunk];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// A few kinds of input, small enough to compress quickly and varied enough
/// that the filters have something to do.
fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    let text: Vec<u8> = std::iter::repeat_n(
        b"the quick brown fox jumps over the lazy dog\n".as_slice(),
        2000,
    )
    .flatten()
    .copied()
    .collect();
    let rand = common::pseudo_random(300_000, 0x5eed);
    let mut mixed = text.clone();
    mixed.extend_from_slice(&rand[..50_000]);
    vec![
        ("empty", Vec::new()),
        ("tiny", b"x".to_vec()),
        ("text", text),
        ("rand", rand),
        ("mixed", mixed),
    ]
}

#[test]
fn committed_vectors_decode_to_their_sources() {
    for source in ["text", "rand", "zeros", "mixed", "tiny", "empty"] {
        for variant in ["p1", "lc1lp1pb0"] {
            let name = format!("{source}.{variant}.xz");
            let path = common::data_dir().join(&name);
            if !path.exists() {
                continue;
            }
            let want = common::read(&format!("src_{source}.bin"));
            let have = decode(&std::fs::read(&path).expect("read")).expect(&name);
            assert_eq!(have, want, "{name}");
            for chunk in [1usize, 3, 1000, 65_536] {
                let have = decode_chunked(&std::fs::read(&path).expect("read"), chunk)
                    .unwrap_or_else(|e| panic!("{name} at chunk {chunk}: {e}"));
                assert_eq!(have, want, "{name} at chunk {chunk}");
            }
        }
    }
}

#[test]
fn every_preset_check_and_filter_round_trips() {
    let cases: &[&[&str]] = &[
        &["-0"],
        &["-6"],
        &["-9", "-e"],
        &["--check=none"],
        &["--check=crc32"],
        &["--check=crc64"],
        &["--check=sha256"],
        &["--delta=dist=1", "--lzma2=preset=6"],
        &["--delta=dist=4", "--lzma2=preset=1"],
        &["--x86", "--lzma2=preset=6"],
        &["--arm", "--lzma2=preset=1"],
        &["--armthumb", "--lzma2=preset=1"],
        &["--arm64", "--lzma2=preset=1"],
        &["--powerpc", "--lzma2=preset=1"],
        &["--sparc", "--lzma2=preset=1"],
        &["--ia64", "--lzma2=preset=1"],
        &["--riscv", "--lzma2=preset=1"],
        &["--x86", "--delta=dist=2", "--lzma2=preset=1"],
        &["--block-size=4096", "-1"],
        &["-T4", "--block-size=8192", "-1"],
    ];
    for (name, data) in payloads() {
        for args in cases {
            let Some(stream) = xz_compress(args, &data) else {
                eprintln!("xz(1) not installed; skipping");
                return;
            };
            let have = decode(&stream).unwrap_or_else(|e| panic!("{name} {args:?}: {e}"));
            assert_eq!(have, data, "{name} {args:?}");
        }
    }
}

#[test]
fn odd_read_sizes_and_filters_agree() {
    let data = payloads()
        .into_iter()
        .find(|(n, _)| *n == "mixed")
        .expect("mixed")
        .1;
    for args in [
        vec!["--x86", "--lzma2=preset=1"],
        vec!["--delta=dist=3", "--lzma2=preset=1"],
        vec!["-T4", "--block-size=8192", "-1"],
    ] {
        let Some(stream) = xz_compress(&args, &data) else {
            return;
        };
        for chunk in [1usize, 2, 7, 13, 4095, 1 << 20] {
            let have = decode_chunked(&stream, chunk)
                .unwrap_or_else(|e| panic!("{args:?} at chunk {chunk}: {e}"));
            assert_eq!(have, data, "{args:?} at chunk {chunk}");
        }
    }
}

#[test]
fn concatenated_streams_and_stream_padding() {
    let a = b"first stream contents, long enough to compress\n".repeat(50);
    let b = b"second stream contents\n".repeat(50);
    let (Some(xa), Some(xb)) = (xz_compress(&["-1"], &a), xz_compress(&["-1"], &b)) else {
        return;
    };

    let mut both = xa.clone();
    both.extend_from_slice(&xb);
    let mut want = a.clone();
    want.extend_from_slice(&b);
    assert_eq!(decode(&both).expect("concatenated"), want);

    // Stream padding: null bytes in whole four-byte groups, before the next
    // stream and at the end of the file.
    let mut padded = xa.clone();
    padded.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
    padded.extend_from_slice(&xb);
    padded.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(decode(&padded).expect("padded"), want);

    // A single-stream reader must refuse what follows the first stream.
    let mut r = XzReader::new(&both[..]).single_stream();
    let mut out = Vec::new();
    let err = r.read_to_end(&mut out).expect_err("trailing data");
    assert_eq!(out, a);
    assert!(format!("{err}").contains("trailing"), "{err}");

    // Padding that is not a multiple of four is not padding.
    let mut ragged = xa.clone();
    ragged.extend_from_slice(&[0, 0]);
    assert!(decode(&ragged).is_err());
}

#[test]
fn truncation_at_every_byte_is_an_error_and_never_a_panic() {
    let data = b"a small but compressible payload\n".repeat(30);
    let Some(stream) = xz_compress(&["-1"], &data) else {
        return;
    };
    for cut in 0..stream.len() {
        let mut out = Vec::new();
        let r = XzReader::new(&stream[..cut]).read_to_end(&mut out);
        assert!(r.is_err(), "truncation at {cut} decoded cleanly");
        assert!(out.len() <= data.len());
        assert_eq!(
            out,
            data[..out.len()],
            "wrong bytes before the cut at {cut}"
        );
    }
}

#[test]
fn single_byte_corruption_is_caught() {
    let data = b"a small but compressible payload\n".repeat(30);
    let Some(stream) = xz_compress(&["-1"], &data) else {
        return;
    };
    let mut caught = 0usize;
    for i in 0..stream.len() {
        for bit in [0x01u8, 0x80] {
            let mut bad = stream.clone();
            bad[i] ^= bit;
            let mut out = Vec::new();
            match XzReader::new(&bad[..]).read_to_end(&mut out) {
                Err(_) => caught += 1,
                // A flip inside the compressed data can still decode to the
                // same bytes only if it was in a field nothing depends on;
                // the check would have caught anything else.
                Ok(_) => assert_eq!(out, data, "corruption at {i} bit {bit:#x} went unnoticed"),
            }
        }
    }
    assert!(caught > stream.len(), "hardly any corruption was caught");
}

#[test]
fn the_memory_limit_is_enforced_and_block_sizes_shrink_it() {
    let data = b"payload\n".repeat(4096);
    let Some(stream) = xz_compress(&["-9"], &data) else {
        return;
    };
    // -9 declares a 64 MiB dictionary, but the block header declares an
    // uncompressed size far below it, so a small limit is still enough.
    let opts = XzOptions::default().with_memory_limit(1 << 20);
    let mut out = Vec::new();
    XzReader::with_options(&stream[..], opts)
        .read_to_end(&mut out)
        .expect("small dictionary");
    assert_eq!(out, data);

    // A limit below even the minimum dictionary is refused rather than
    // rounded up.
    let opts = XzOptions::default().with_memory_limit(16);
    let err = XzReader::with_options(&stream[..], opts)
        .read_to_end(&mut Vec::new())
        .expect_err("limit");
    assert!(format!("{err}").contains("limit"), "{err}");
}

#[test]
fn an_output_cap_stops_a_decode() {
    let data = b"payload\n".repeat(4096);
    let Some(stream) = xz_compress(&["-1"], &data) else {
        return;
    };
    let opts = XzOptions::default().with_max_unpack_bytes(Some(100));
    let mut out = Vec::new();
    let err = XzReader::with_options(&stream[..], opts)
        .read_to_end(&mut out)
        .expect_err("cap");
    assert!(format!("{err}").contains("cap"), "{err}");
    assert!(out.len() <= 100);
}
