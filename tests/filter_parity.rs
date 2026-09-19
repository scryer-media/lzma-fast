//! Bit-exactness of the BCJ and delta filters against the reference SDK.
//!
//! `cargo xtask lzma-util` builds `filter-oracle` from the pinned SDK's own
//! `Bra.c`, `Bra86.c`, `BraIA64.c` and `Delta.c` into `target/lzma-util`. Both
//! directions of every converter are compared against it byte for byte. As
//! with the other parity tests, a missing binary is a skip unless
//! `LZMA_TURBO_LZMA_UTIL_REQUIRE` is set.
//!
//! Random bytes rarely contain the instructions a branch converter looks for,
//! so the corpus here is generated code-like data: the opcode each converter
//! recognises, planted at its alignment among random bytes, plus the shared
//! corpus so the boundary cases are covered too.

#![cfg(all(feature = "std", feature = "xz"))]

use std::process::Command;

use lzma_turbo::xz::bcj::{Bcj, BcjKind};
use lzma_turbo::xz::delta::Delta;

mod corpus;

use corpus::{corpus, tempdir, tool};

// ---------------------------------------------------------------------------
// Code-like inputs.
// ---------------------------------------------------------------------------

/// A xorshift, so the inputs are generated rather than committed.
struct Rng(u64);

impl Rng {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }
}

/// Bytes that look like the given architecture: every few instruction slots
/// carries a branch the converter will act on.
fn code_like(kind: BcjKind, len: usize, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    let mut out = vec![0u8; len];
    for b in out.iter_mut() {
        *b = rng.next_u32() as u8;
    }
    let step = kind.alignment().max(1) as usize;
    let mut i = 0usize;
    while i + 8 <= len {
        if rng.next_u32().is_multiple_of(3) {
            let target = rng.next_u32();
            match kind {
                // A near CALL, and the 0xE8 the x86 converter scans for.
                BcjKind::X86 => {
                    out[i] = 0xE8;
                    out[i + 1..i + 5].copy_from_slice(&target.to_le_bytes());
                }
                // BL, and ADRP with its following instruction.
                BcjKind::Arm64 => {
                    let v = if rng.next_u32() & 1 == 0 {
                        0x9400_0000 | (target & 0x03FF_FFFF)
                    } else {
                        0x9000_0000 | (target & 0x00FF_FFFF)
                    };
                    out[i..i + 4].copy_from_slice(&v.to_le_bytes());
                }
                // BL: 0xEB in the top byte.
                BcjKind::Arm => {
                    out[i..i + 3].copy_from_slice(&target.to_le_bytes()[..3]);
                    out[i + 3] = 0xEB;
                }
                // BL: 0x480000001 with the link bit.
                BcjKind::Ppc => {
                    let v = 0x4800_0001 | (target & 0x03FF_FFFC);
                    out[i..i + 4].copy_from_slice(&v.to_be_bytes());
                }
                // CALL: the 0x40000000 form the converter matches.
                BcjKind::Sparc => {
                    let v = 0x4000_0000 | (target & 0x003F_FFFF);
                    out[i..i + 4].copy_from_slice(&v.to_be_bytes());
                }
                // The Thumb BL pair: 0xF000 then 0xF800.
                BcjKind::ArmThumb => {
                    let hi = 0xF000u16 | ((target >> 12) & 0x7FF) as u16;
                    let lo = 0xF800u16 | (target & 0x7FF) as u16;
                    out[i..i + 2].copy_from_slice(&hi.to_le_bytes());
                    out[i + 2..i + 4].copy_from_slice(&lo.to_le_bytes());
                }
                // A bundle whose template byte selects a slot with a long
                // branch, which is what the converter's table tests for.
                BcjKind::Ia64 => {
                    out[i] = (rng.next_u32() % 32) as u8 & 0x1E;
                    for j in 1..16 {
                        out[i + j] = rng.next_u32() as u8;
                    }
                }
                // JAL and AUIPC, the two the converter scans for.
                BcjKind::RiscV => {
                    let v = if rng.next_u32() & 1 == 0 {
                        0x6F | (target & 0xFFFF_F000)
                    } else {
                        0x17 | (target & 0xFFFF_F000)
                    };
                    out[i..i + 4].copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        i += step.max(4);
    }
    out
}

fn inputs(kind: BcjKind) -> Vec<(String, Vec<u8>)> {
    let mut cases: Vec<(String, Vec<u8>)> = corpus();
    for (tag, len) in [("code-small", 1024usize), ("code-big", 200_003)] {
        cases.push((
            tag.to_owned(),
            code_like(kind, len, 0x5DEE_CE66 ^ len as u64),
        ));
    }
    cases
}

// ---------------------------------------------------------------------------
// The oracle.
// ---------------------------------------------------------------------------

fn oracle(
    bin: &std::path::Path,
    dir: &std::path::Path,
    name: &str,
    dir_flag: &str,
    n: u32,
    data: &[u8],
) -> Vec<u8> {
    let src = dir.join("in.bin");
    let dst = dir.join("out.bin");
    std::fs::write(&src, data).unwrap();
    let _ = std::fs::remove_file(&dst);
    let st = Command::new(bin)
        .args([
            name,
            dir_flag,
            &n.to_string(),
            src.to_str().unwrap(),
            dst.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(st.success(), "filter-oracle {name} {dir_flag} failed");
    std::fs::read(&dst).unwrap()
}

const KINDS: [(BcjKind, &str); 8] = [
    (BcjKind::X86, "x86"),
    (BcjKind::Ppc, "ppc"),
    (BcjKind::Ia64, "ia64"),
    (BcjKind::Arm, "arm"),
    (BcjKind::ArmThumb, "armt"),
    (BcjKind::Sparc, "sparc"),
    (BcjKind::Arm64, "arm64"),
    (BcjKind::RiscV, "riscv"),
];

#[test]
fn every_bcj_converter_matches_the_sdk_byte_for_byte() {
    let Some(bin) = tool("filter-oracle") else {
        return;
    };
    let dir = tempdir("bcj-parity");
    for (kind, name) in KINDS {
        let cases = inputs(kind);
        for start in [0u32, kind.alignment() * 3, 0x1000] {
            for (tag, data) in &cases {
                for encoding in [true, false] {
                    let flag = if encoding { "enc" } else { "dec" };
                    let want = oracle(&bin, &dir, name, flag, start, data);
                    let mut ours = data.clone();
                    let mut bcj = Bcj::new(kind, start).expect("aligned");
                    if encoding {
                        bcj.encode(&mut ours);
                    } else {
                        bcj.decode(&mut ours);
                    }
                    assert_eq!(ours, want, "{name} {flag} start={start} case {tag}");
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_delta_filter_matches_the_sdk_byte_for_byte() {
    let Some(bin) = tool("filter-oracle") else {
        return;
    };
    let dir = tempdir("delta-parity");
    for distance in [1u32, 2, 3, 4, 16, 255, 256] {
        for (tag, data) in corpus() {
            for encoding in [true, false] {
                let flag = if encoding { "enc" } else { "dec" };
                let want = oracle(&bin, &dir, "delta", flag, distance, &data);
                let mut ours = data.clone();
                let mut d = Delta::new((distance - 1) as u8).expect("distance");
                if encoding {
                    d.encode(&mut ours);
                } else {
                    d.decode(&mut ours);
                }
                assert_eq!(ours, want, "delta {flag} distance={distance} case {tag}");
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn feeding_a_converter_in_pieces_gives_what_feeding_it_whole_gives() {
    // The carry contract: a converter consumes a prefix and leaves its tail
    // for the next call, so chunked encoding must equal whole-buffer
    // encoding. This is what lets the .xz writer filter a block in one go and
    // the reader undo it chunk by chunk.
    for (kind, name) in KINDS {
        let data = code_like(kind, 40_000, 0xC0FF_EE01);
        let mut whole = data.clone();
        Bcj::new(kind, 0).unwrap().encode(&mut whole);

        for chunk in [1usize, 3, 7, 16, 4096] {
            let mut bcj = Bcj::new(kind, 0).unwrap();
            let mut out: Vec<u8> = Vec::new();
            let mut carry: Vec<u8> = Vec::new();
            let mut rest = &data[..];
            while !rest.is_empty() {
                let take = chunk.min(rest.len());
                carry.extend_from_slice(&rest[..take]);
                rest = &rest[take..];
                let n = bcj.encode(&mut carry);
                out.extend_from_slice(&carry[..n]);
                carry.drain(..n);
            }
            out.extend_from_slice(&carry);
            assert_eq!(out.len(), data.len(), "{name} chunk={chunk}");
            // The tail a converter never got to see stays as it was, which is
            // exactly what the whole-buffer call leaves too.
            assert_eq!(out, whole, "{name} chunk={chunk}");
        }
    }
}
