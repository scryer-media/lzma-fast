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

#![cfg(feature = "std")]

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use lzma_turbo::{LzmaEncProps, LzmaEncoder, MatchFinderKind, SliceStream};

// ---------------------------------------------------------------------------
// The corpus.
// ---------------------------------------------------------------------------

/// The same SplitMix64 `xtask/src/vectors.rs` generates the decoder vectors
/// with, so one seed means one corpus on every machine.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

fn random(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    (0..len).map(|_| rng.next_u64() as u8).collect()
}

fn text(seed: u64, len: usize) -> Vec<u8> {
    const WORDS: &[&str] = &[
        "the",
        "quick",
        "brown",
        "fox",
        "jumps",
        "over",
        "lazy",
        "dog",
        "lorem",
        "ipsum",
        "dolor",
        "sit",
        "amet",
        "consectetur",
        "adipiscing",
        "elit",
    ];
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        out.extend_from_slice(WORDS[rng.below(WORDS.len())].as_bytes());
        out.push(if rng.below(10) == 0 { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

/// Runs of one byte and short repeated phrases: the shape that makes the
/// optimal parser take its rep and short-rep branches.
fn repeats(seed: u64, len: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(len + 512);
    while out.len() < len {
        match rng.below(3) {
            0 => {
                let b = rng.next_u64() as u8;
                out.extend(std::iter::repeat_n(b, 1 + rng.below(400)));
            }
            1 => {
                let phrase = random(rng.next_u64(), 3 + rng.below(12));
                for _ in 0..1 + rng.below(40) {
                    out.extend_from_slice(&phrase);
                }
            }
            _ => out.extend_from_slice(&random(rng.next_u64(), 1 + rng.below(64))),
        }
    }
    out.truncate(len);
    out
}

fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut cases: Vec<(String, Vec<u8>)> = vec![
        ("empty".into(), Vec::new()),
        ("one-byte".into(), vec![0x2A]),
        ("two-bytes".into(), vec![0xFF, 0xFF]),
        ("zeros-1".into(), vec![0; 1]),
        ("zeros-65536".into(), vec![0; 65536]),
        // 1 << 16 is LZMA2_PACK_SIZE_MAX and the range encoder's buffer, and
        // 1 << 21 is LZMA2_UNPACK_SIZE_MAX; sit either side of both.
        ("zeros-65535".into(), vec![0; 65535]),
        ("zeros-65537".into(), vec![0; 65537]),
    ];
    for (name, len) in [
        ("tiny", 7usize),
        ("small", 999),
        ("chunk-under", (1 << 16) - 1),
        ("chunk-over", (1 << 16) + 1),
        ("big", 300_000),
    ] {
        cases.push((format!("random-{name}"), random(0x1234 ^ len as u64, len)));
        cases.push((format!("text-{name}"), text(0x5678 ^ len as u64, len)));
        cases.push((format!("repeats-{name}"), repeats(0x9ABC ^ len as u64, len)));
    }
    cases
}

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
    out
}

// ---------------------------------------------------------------------------
// The reference binaries.
// ---------------------------------------------------------------------------

fn tools_dir() -> PathBuf {
    std::env::var_os("LZMA_TURBO_LZMA_UTIL").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join("lzma-util")
        },
        PathBuf::from,
    )
}

fn required() -> bool {
    std::env::var_os("LZMA_TURBO_LZMA_UTIL_REQUIRE").is_some()
}

fn tool(name: &str) -> Option<PathBuf> {
    let path = tools_dir().join(name);
    if path.is_file() {
        return Some(path);
    }
    assert!(
        !required(),
        "LZMA_TURBO_LZMA_UTIL_REQUIRE is set but {} is missing; run `cargo xtask lzma-util`",
        path.display()
    );
    eprintln!(
        "skipping: {} not built; run `cargo xtask lzma-util`",
        path.display()
    );
    None
}

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

fn tempdir(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "lzma-turbo-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}
