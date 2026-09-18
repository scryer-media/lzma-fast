//! The `.xz` writer: what it produces must come back through every reader
//! this crate has, and through `xz` itself.
//!
//! The parity tests prove the compressed data is bit-exact with the SDK's
//! encoder. Nothing proves the *frame* that way, because the SDK's `.xz`
//! writer is not what was ported, so it is proved the other way round: by
//! decoding.

#![cfg(all(feature = "enc", feature = "xz"))]

use std::io::{Cursor, Read, Write};
use std::process::{Command, Stdio};

use lzma_turbo::xz::{CheckType, XzAdaptiveDecoder, XzOptions, XzParallelReader, XzReader};
use lzma_turbo::{DrainStatus, LzmaEncProps, LzmaWriter, XzWriter, encode_lzma_alone, encode_xz};

mod corpus;
use corpus::{corpus, tempdir};

/// Every check type this build can write.
fn checks() -> Vec<CheckType> {
    let mut v = vec![CheckType::None, CheckType::Crc32, CheckType::Crc64];
    if cfg!(any(feature = "crypto", feature = "native-crypto")) {
        v.push(CheckType::Sha256);
    }
    v
}

/// Drives [`XzAdaptiveDecoder`] over a whole stream, as `tests/xz_utils.rs`
/// does.
fn adaptive(data: &[u8], threads: usize, chunk: usize) -> Vec<u8> {
    let mut dec = XzAdaptiveDecoder::new(XzOptions::default().with_threads(threads));
    let mut out = Vec::new();
    let mut pos = 0;
    if data.is_empty() {
        dec.end_of_input();
    }
    loop {
        if pos < data.len() {
            pos += dec
                .feed(&data[pos..(pos + chunk).min(data.len())])
                .expect("feed");
            if pos == data.len() {
                dec.end_of_input();
            }
        }
        let status = dec
            .drain(|off, bytes| {
                let off = usize::try_from(off).expect("offset");
                if out.len() < off + bytes.len() {
                    out.resize(off + bytes.len(), 0);
                }
                out[off..off + bytes.len()].copy_from_slice(bytes);
            })
            .expect("drain");
        match status {
            DrainStatus::Finished => return out,
            DrainStatus::NeedsMoreInput if pos == data.len() => {
                panic!("the adaptive decoder wanted input after the stream ended")
            }
            _ => {}
        }
    }
}

/// Puts one stream through all three readers.
fn decode_every_way(xz: &[u8], want: &[u8], what: &str) {
    let mut out = Vec::new();
    XzReader::new(Cursor::new(xz))
        .read_to_end(&mut out)
        .unwrap_or_else(|e| panic!("{what}: XzReader: {e}"));
    assert_eq!(out, want, "{what}: XzReader");

    let mut out = Vec::new();
    XzParallelReader::with_options(Cursor::new(xz), XzOptions::default().with_threads(4))
        .unwrap_or_else(|e| panic!("{what}: XzParallelReader: {e}"))
        .read_to_end(&mut out)
        .unwrap_or_else(|e| panic!("{what}: XzParallelReader: {e}"));
    assert_eq!(out, want, "{what}: XzParallelReader");

    assert_eq!(adaptive(xz, 1, 4096), want, "{what}: adaptive, 1 thread");
    // A byte-at-a-time feed is the interesting case for the adaptive
    // decoder's state machine and a slow one for a large stream, so the
    // small inputs carry it.
    if xz.len() < 64 * 1024 {
        assert_eq!(
            adaptive(xz, 4, 7),
            want,
            "{what}: adaptive, 4 threads, 7-byte feeds"
        );
    }
}

#[test]
fn every_check_and_block_size_round_trips_through_every_reader() {
    for (name, data) in corpus() {
        for check in checks() {
            for block_size in [0u64, 4096, 1 << 16] {
                let props = LzmaEncProps::new().with_level(3).with_dict_size(1 << 16);
                let xz = encode_xz(&data, &props, check, block_size).expect("encode");
                decode_every_way(
                    &xz,
                    &data,
                    &format!("{name}, check {check:?}, block {block_size}"),
                );
            }
        }
    }
}

#[test]
fn the_writer_adapters_produce_the_same_bytes_as_the_one_shot_calls() {
    let props = LzmaEncProps::new().with_level(4);
    let data: Vec<u8> = (0..200_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();

    let mut w = XzWriter::new(Vec::new(), &props).expect("writer");
    w.set_check(CheckType::Crc64).expect("check");
    w.set_block_size(1 << 16);
    for chunk in data.chunks(1777) {
        w.write_all(chunk).expect("write");
    }
    let streamed = w.finish().expect("finish");
    let one_shot = encode_xz(&data, &props, CheckType::Crc64, 1 << 16).expect("encode");
    assert_eq!(streamed, one_shot, "streaming must not change the stream");
    decode_every_way(&streamed, &data, "XzWriter");

    let mut w = LzmaWriter::new(Vec::new(), &props);
    for chunk in data.chunks(1777) {
        w.write_all(chunk).expect("write");
    }
    assert_eq!(
        w.finish().expect("finish"),
        encode_lzma_alone(&data, &props).expect("encode"),
        "LzmaWriter"
    );
}

/// Is `xz` on PATH, and does it run?
fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Runs `tool` with `args` over `input` on stdin and returns stdout.
///
/// The write goes on its own thread: the decompressed output is much larger
/// than the input, so a parent that writes it all before reading fills the
/// child's stdout pipe and both ends stop.
fn pipe(tool: &str, args: &[&str], input: &[u8]) -> Option<Vec<u8>> {
    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take().expect("stdin");
    let owned = input.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&owned);
    });
    let out = child.wait_with_output().expect("wait");
    writer.join().expect("writer");
    out.status.success().then_some(out.stdout)
}

#[test]
fn xz_itself_accepts_what_this_crate_writes() {
    if !have("xz") {
        eprintln!("skipping: `xz` is not on PATH");
        return;
    }
    let dir = tempdir("xz-encoder");
    for (name, data) in corpus() {
        for check in checks() {
            let props = LzmaEncProps::new().with_level(3).with_dict_size(1 << 16);
            let xz = encode_xz(&data, &props, check, 4096).expect("encode");

            // `xz -t` checks the framing, the index and the checks without
            // producing output.
            let path = dir.join(format!("{name}-{check:?}.xz"));
            std::fs::write(&path, &xz).expect("write");
            let status = Command::new("xz")
                .arg("-t")
                .arg(&path)
                .status()
                .expect("run xz -t");
            assert!(status.success(), "xz -t rejected {name}, check {check:?}");

            let got = pipe("xz", &["-dc"], &xz)
                .unwrap_or_else(|| panic!("xz -dc failed on {name}, check {check:?}"));
            assert_eq!(got, data, "xz -dc on {name}, check {check:?}");
        }

        // The `.lzma` writer goes through the same tool, which reads
        // LZMA-Alone with `--format=lzma`.
        let alone = encode_lzma_alone(&data, &LzmaEncProps::new()).expect("encode");
        let got = pipe("xz", &["-dc", "--format=lzma"], &alone)
            .unwrap_or_else(|| panic!("xz -dc --format=lzma failed on {name}"));
        assert_eq!(got, data, "xz -dc --format=lzma on {name}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn seven_zip_accepts_what_this_crate_writes() {
    if !have("7zz") {
        eprintln!("skipping: `7zz` is not on PATH");
        return;
    }
    let dir = tempdir("7zz-encoder");
    for (name, data) in corpus() {
        let props = LzmaEncProps::new().with_level(3).with_dict_size(1 << 16);
        let path = dir.join(format!("{name}.xz"));
        std::fs::write(
            &path,
            encode_xz(&data, &props, CheckType::Crc64, 0).expect("encode"),
        )
        .expect("write");
        let out = Command::new("7zz")
            .arg("t")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run 7zz");
        assert!(out.success(), "7zz t rejected {name}");
    }
    std::fs::remove_dir_all(&dir).ok();
}
