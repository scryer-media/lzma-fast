//! The generated corpus the encoder tests encode.
//!
//! Nothing here is committed as bytes: everything comes out of one seeded
//! `SplitMix64`, the same generator `xtask/src/vectors.rs` builds the decoder
//! vectors with, so one seed means one corpus on every machine.

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

pub fn corpus() -> Vec<(String, Vec<u8>)> {
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
// The reference binaries `cargo xtask lzma-util` builds.
// ---------------------------------------------------------------------------

use std::path::{Path, PathBuf};

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

/// The path to one of them, or `None` with a message — unless
/// `LZMA_TURBO_LZMA_UTIL_REQUIRE` is set, as CI sets it, and then a missing
/// binary is a failure.
pub fn tool(name: &str) -> Option<PathBuf> {
    let path = tools_dir().join(name);
    if path.is_file() {
        return Some(path);
    }
    assert!(
        std::env::var_os("LZMA_TURBO_LZMA_UTIL_REQUIRE").is_none(),
        "LZMA_TURBO_LZMA_UTIL_REQUIRE is set but {} is missing; run `cargo xtask lzma-util`",
        path.display()
    );
    eprintln!(
        "skipping: {} not built; run `cargo xtask lzma-util`",
        path.display()
    );
    None
}

/// A directory the reference binaries can write into.
pub fn tempdir(tag: &str) -> PathBuf {
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
