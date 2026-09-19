//! Bit-exactness against the reference LZMA encoder.
//!
//! `cargo xtask lzma-util` builds two binaries from the pinned LZMA SDK into
//! `target/lzma-util` (or wherever `LZMA_TURBO_LZMA_UTIL` points):
//! `lzma`, `C/Util/Lzma/LzmaUtil.c` as it ships, and `lzma-oracle`, a small
//! harness that exposes the settings `LzmaUtil` hard-codes. Without them this
//! test says so and passes, unless `LZMA_TURBO_LZMA_UTIL_REQUIRE` is set, as
//! CI sets it.
//!
//! Every case is generated here, never committed: the corpus is a seeded
//! `SplitMix64` away from a handful of shapes, sized to straddle the
//! dictionary sizes and the 64 KiB range-encoder buffer.

#![cfg(all(feature = "std", feature = "enc"))]

use std::process::Command;

use lzma_turbo::{LzmaEncProps, LzmaEncoder, MatchFinderKind, SliceStream};

mod corpus;

use corpus::{corpus, tempdir, tool};

// ---------------------------------------------------------------------------
// The settings.
// ---------------------------------------------------------------------------

struct Setting {
    level: u32,
    kind: MatchFinderKind,
    lc: u8,
    lp: u8,
    pb: u8,
    fb: u32,
    dict_size: u32,
}

impl Setting {
    fn oracle_args(&self) -> Vec<String> {
        let (bt, nh) = (self.kind.bt_mode(), self.kind.num_hash_bytes());
        [
            self.level,
            u32::from(bt),
            nh,
            u32::from(self.lc),
            u32::from(self.lp),
            u32::from(self.pb),
            self.fb,
            self.dict_size,
        ]
        .iter()
        .map(u32::to_string)
        .collect()
    }

    /// The same, for `lzma-oracle-mt`, which takes `numThreads` as well.
    fn oracle_mt_args(&self, threads: u32) -> Vec<String> {
        let mut args = self.oracle_args();
        args.push(threads.to_string());
        args
    }

    fn props(&self) -> LzmaEncProps {
        LzmaEncProps::new()
            .with_level(self.level)
            .with_match_finder(self.kind)
            .with_lclppb(self.lc, self.lp, self.pb)
            .with_fast_bytes(self.fb)
            .with_dict_size(self.dict_size)
    }

    fn name(&self) -> String {
        format!(
            "level={} mf={:?} lc={} lp={} pb={} fb={} dict={}",
            self.level, self.kind, self.lc, self.lp, self.pb, self.fb, self.dict_size
        )
    }
}

fn settings() -> Vec<Setting> {
    let mut out = Vec::new();
    // One setting per match finder, at the level that selects it, plus the
    // levels either side of the fast/optimal parser split at 5.
    for kind in [
        MatchFinderKind::Hc4,
        MatchFinderKind::Hc5,
        MatchFinderKind::Bt2,
        MatchFinderKind::Bt3,
        MatchFinderKind::Bt4,
        MatchFinderKind::Bt5,
    ] {
        for level in [0, 4, 5, 9] {
            out.push(Setting {
                level,
                kind,
                lc: 3,
                lp: 0,
                pb: 2,
                fb: 32,
                dict_size: 1 << 20,
            });
        }
    }
    // Literal and position context, and the two ends of the fast-byte range.
    for (lc, lp, pb) in [(0, 0, 0), (0, 2, 0), (4, 0, 4), (1, 1, 1), (8, 0, 0)] {
        out.push(Setting {
            level: 6,
            kind: MatchFinderKind::Bt4,
            lc,
            lp,
            pb,
            fb: 64,
            dict_size: 1 << 20,
        });
    }
    for fb in [5, 273] {
        out.push(Setting {
            level: 5,
            kind: MatchFinderKind::Bt4,
            lc: 3,
            lp: 0,
            pb: 2,
            fb,
            dict_size: 1 << 20,
        });
    }
    // Dictionaries smaller than the corpus, which is what makes the match
    // finder move its block and normalize its hash.
    for dict_size in [1 << 12, 1 << 16] {
        for kind in [MatchFinderKind::Hc5, MatchFinderKind::Bt4] {
            out.push(Setting {
                level: 9,
                kind,
                lc: 3,
                lp: 0,
                pb: 2,
                fb: 32,
                dict_size,
            });
        }
    }
    // A dictionary big enough that `hashMask >= 0xFFFFFF`, which is what sets
    // `MFB.bigHash` and swaps `GetHeads4`/`GetHeads5` for `GetHeads4b`/`5b` in
    // the threaded finder. Nothing here tells the encoder the input size, so
    // the hash is not reduced back down.
    for kind in [MatchFinderKind::Bt4, MatchFinderKind::Bt5] {
        out.push(Setting {
            level: 9,
            kind,
            lc: 3,
            lp: 0,
            pb: 2,
            fb: 32,
            dict_size: 1 << 26,
        });
    }
    out
}

// ---------------------------------------------------------------------------
// The reference binaries.
// ---------------------------------------------------------------------------

/// The `.lzma` header the reference writes: five property bytes then the
/// uncompressed size, little-endian.
fn our_alone(src: &[u8], setting: &Setting) -> Vec<u8> {
    let mut enc = LzmaEncoder::new(&setting.props()).expect("encoder");
    let mut out = Vec::new();
    out.extend_from_slice(&enc.properties());
    out.extend_from_slice(&(src.len() as u64).to_le_bytes());
    let mut input = SliceStream::new(src);
    enc.encode(&mut input, &mut out).expect("encode");
    out
}

/// The same encoding, with `numThreads = 2`, which turns on the threaded match
/// finder wherever `mtMode` allows it: `btMode && !fastMode`. C:
/// `p->mtMode = (p->multiThread && !p->fastMode && (MFB.btMode != 0))`.
fn our_alone_mt(src: &[u8], setting: &Setting) -> Vec<u8> {
    let props = setting.props().with_num_threads(2);
    let mut enc = LzmaEncoder::new(&props).expect("encoder");
    let mut out = Vec::new();
    out.extend_from_slice(&enc.properties());
    out.extend_from_slice(&(src.len() as u64).to_le_bytes());
    let mut input = SliceStream::new(src);
    enc.encode_send(&mut input, &mut out).expect("encode");
    out
}

#[test]
fn matches_the_reference_encoder_at_every_setting() {
    let Some(oracle) = tool("lzma-oracle") else {
        return;
    };
    let dir = tempdir("lzma-parity");
    let src_path = dir.join("in.bin");
    let ref_path = dir.join("ref.lzma");

    let settings = settings();
    let mut compared = 0usize;
    for (name, src) in corpus() {
        std::fs::write(&src_path, &src).unwrap();
        for setting in &settings {
            let status = Command::new(&oracle)
                .args(setting.oracle_args())
                .arg(&src_path)
                .arg(&ref_path)
                .status()
                .expect("run the reference encoder");
            assert!(
                status.success(),
                "reference failed on {name} [{}]",
                setting.name()
            );
            let expected = std::fs::read(&ref_path).unwrap();
            let got = our_alone(&src, setting);
            assert!(
                expected == got,
                "not bit-exact on {name} [{}]: reference {} bytes, ours {} bytes",
                setting.name(),
                expected.len(),
                got.len()
            );
            compared += 1;
        }
    }
    assert!(compared > 0);
    eprintln!("compared {compared} encodings against the reference");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The threaded match finder, against the same SDK built without `Z7_ST`.
///
/// The SDK's own `LzFindMt.c` does not produce the same stream as `LzFind.c`:
/// `Bt5_MatchFinder_GetMatches` extends its hash match past `numHashBytes`
/// with `UPDATE_maxLen` and hands that length to the binary tree, while
/// `MixMatches4` stops at 4 and the bt thread's `GetMatchesSpecN_2` always
/// starts from `numHashBytes - 1`. So the reference for this lane is the C
/// with the threaded finder compiled in, not the single-threaded oracle.
#[test]
fn matches_the_threaded_reference_at_every_setting() {
    let Some(oracle) = tool("lzma-oracle-mt") else {
        return;
    };
    let dir = tempdir("lzma-parity-mt");
    let src_path = dir.join("in.bin");
    let ref_path = dir.join("ref.lzma");

    let settings = settings();
    let mut compared = 0usize;
    for (name, src) in corpus() {
        std::fs::write(&src_path, &src).unwrap();
        for setting in &settings {
            let status = Command::new(&oracle)
                .args(setting.oracle_mt_args(2))
                .arg(&src_path)
                .arg(&ref_path)
                .status()
                .expect("run the threaded reference encoder");
            assert!(
                status.success(),
                "reference failed on {name} [{}]",
                setting.name()
            );
            let expected = std::fs::read(&ref_path).unwrap();
            let got = our_alone_mt(&src, setting);
            assert!(
                expected == got,
                "not bit-exact on {name} [{}]: reference {} bytes, ours {} bytes",
                setting.name(),
                expected.len(),
                got.len()
            );
            compared += 1;
        }
    }
    assert!(compared > 0);
    eprintln!("compared {compared} threaded encodings against the reference");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `LzmaUtil` as it ships, which is the encoder as an ordinary user of the SDK
/// gets it: `LzmaEncProps_Init` defaults, nothing set.
#[test]
fn matches_lzma_util_as_it_ships() {
    let Some(util) = tool("lzma") else {
        return;
    };
    let dir = tempdir("lzma-util");
    let src_path = dir.join("in.bin");
    let ref_path = dir.join("ref.lzma");

    let default = Setting {
        level: 5,
        kind: MatchFinderKind::Bt4,
        lc: 3,
        lp: 0,
        pb: 2,
        fb: 32,
        // LzmaEncProps_Normalize's level-5 default on a 64-bit host:
        // 1 << (level + 20), since 5 <= sizeof(size_t) / 2 + 4.
        dict_size: 1 << 25,
    };

    for (name, src) in corpus() {
        std::fs::write(&src_path, &src).unwrap();
        let status = Command::new(&util)
            .arg("e")
            .arg(&src_path)
            .arg(&ref_path)
            .status()
            .expect("run LzmaUtil");
        assert!(status.success(), "LzmaUtil failed on {name}");
        assert!(
            std::fs::read(&ref_path).unwrap() == our_alone(&src, &default),
            "not bit-exact with LzmaUtil on {name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Everything the encoder writes comes back through this crate's own decoder.
#[test]
fn round_trips_through_the_decoder() {
    use lzma_turbo::LzmaReader;

    let corpus = corpus();
    for setting in settings() {
        for (name, src) in &corpus {
            let encoded = our_alone(src, &setting);
            let mut reader =
                LzmaReader::new(std::io::Cursor::new(&encoded)).expect("reader over .lzma");
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut reader, &mut out).expect("decode");
            assert!(
                &out == src,
                "round trip lost bytes on {name} [{}]",
                setting.name()
            );
        }
    }
}
