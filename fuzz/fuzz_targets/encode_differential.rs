//! Arbitrary bytes through this crate's encoder and the LZMA SDK's at the
//! same settings: the two must produce the same bytes.
//!
//! `encode_round_trip` asks only that what this crate writes, it can read
//! back; the parity tests ask for bit-exactness against the SDK, but over a
//! fixed corpus at a fixed list of settings. This is the two put together:
//! the fuzzer picks the input *and* the settings, and anything less than a
//! byte-for-byte match is a finding.
//!
//! The SDK side is `tools/sdk-encoder`, which links `LzmaEnc.c`,
//! `Lzma2Enc.c`, `LzFindMt.c` and `MtCoder.c` from the pinned checkout into
//! this binary - not the command-line oracles the parity tests drive, which
//! would spend the fuzzer's budget on processes and temporary files. Without
//! `LZMA_SDK` at build time there is no SDK here and the target does nothing;
//! CI sets `LZMA_ENCODER_ORACLE_REQUIRE=1` so that cannot pass unnoticed.
//!
//! Both threaded paths are in: the match finder's threads, which change the
//! bytes deterministically and identically on both sides, and the block
//! threads, which must not change them at all.
#![no_main]

use libfuzzer_sys::fuzz_target;
use lzma_turbo::xz::bcj::{Bcj, BcjKind};
use lzma_turbo::xz::delta::Delta;
use lzma_turbo::{
    BLOCK_SIZE_SOLID, LzmaEncProps, MatchFinderKind, encode_lzma2_mt, encode_lzma_alone,
};
use sdk_encoder::Props;

/// Past this the case is truncated. A fuzzer is for the shapes, not the
/// sizes, and level 9 over a megabyte is one case per second.
const MAX_INPUT: usize = 1 << 16;

/// The block sizes, as the C numbers them: 0 is solid.
const BLOCKS: [u64; 4] = [0, 1 << 14, 1 << 16, 100_000];

fuzz_target!(|data: &[u8]| {
    if !sdk_encoder::AVAILABLE {
        return;
    }
    let [a, b, c, d, e, f, src @ ..] = data else {
        return;
    };
    let src = &src[..src.len().min(MAX_INPUT)];

    let finder = match b % 6 {
        0 => MatchFinderKind::Hc4,
        1 => MatchFinderKind::Hc5,
        2 => MatchFinderKind::Bt2,
        3 => MatchFinderKind::Bt3,
        4 => MatchFinderKind::Bt4,
        _ => MatchFinderKind::Bt5,
    };
    // lc + lp must not exceed 4, and both encoders refuse the same settings;
    // picking from the legal set spends the budget on bytes instead.
    let lc = *d % 5;
    let lp = (*d >> 3) % (5 - lc);
    let pb = *e % 5;
    let props = LzmaEncProps::new()
        .with_level(u32::from(*a) % 10)
        .with_match_finder(finder)
        .with_lclppb(lc, lp, pb)
        // The C's range is 5..=273.
        .with_fast_bytes(5 + (u32::from(*e) * 7 + u32::from(*d)) % 269)
        // A small dictionary keeps memory flat and is where the window moves.
        .with_dict_size(1 << (12 + u32::from(*c) % 7));
    // 2 is the threaded match finder; it changes the bytes, in both.
    let mf_threads = if f & 1 == 0 { 1 } else { 2 };
    let props = props.with_num_threads(mf_threads);

    let n = props.normalized();
    let sdk = Props {
        level: n.level as i32,
        bt_mode: n.bt_mode as i32,
        num_hash_bytes: n.num_hash_bytes as i32,
        lc: n.lc as i32,
        lp: n.lp as i32,
        pb: n.pb as i32,
        fb: n.fb,
        dict_size: n.dict_size,
        num_threads: mf_threads as i32,
    };

    if f & 2 == 0 {
        // LZMA-Alone: the 13-byte header and the stream.
        let ours = encode_lzma_alone(src, &props).expect("our own settings encode");
        let theirs = sdk_encoder::lzma1(src, &sdk)
            .expect("the SDK is linked")
            .expect("the SDK takes the same settings");
        same("lzma1", &props, mf_threads, 0, 1, src.len(), &ours, &theirs);
    } else {
        // Raw LZMA2, at a block size and a block thread count. The block
        // threads must not reach the bytes, which is why the same case is
        // also encoded with one.
        let block_size = BLOCKS[usize::from(*f >> 2) % BLOCKS.len()];
        let block_threads = usize::from(*c % 4) + 1;
        let ours_block = if block_size == 0 {
            BLOCK_SIZE_SOLID
        } else {
            block_size
        };
        let (our_prop, ours) =
            encode_lzma2_mt(src, &props, ours_block, block_threads).expect("our own settings");
        let (their_prop, theirs) = sdk_encoder::lzma2(src, &sdk, block_size, block_threads as i32)
            .expect("the SDK is linked")
            .expect("the SDK takes the same settings");
        assert_eq!(
            our_prop, their_prop,
            "the LZMA2 property byte differs: dict {}",
            n.dict_size
        );
        same(
            "lzma2",
            &props,
            mf_threads,
            block_size,
            block_threads,
            src.len(),
            &ours,
            &theirs,
        );
    }

    // The two filters that sit in front of the coder in an .xz chain, over
    // the same bytes. `distance` is the delta filter's 1..=256.
    let distance = u32::from(*b) + 1;
    let mut ours = src.to_vec();
    Delta::new((distance - 1) as u8)
        .expect("1..=256")
        .encode(&mut ours);
    let mut theirs = src.to_vec();
    assert!(sdk_encoder::delta_encode(&mut theirs, distance));
    assert!(ours == theirs, "the delta filter differs at distance {distance}");

    let mut ours = src.to_vec();
    Bcj::new(BcjKind::X86, 0)
        .expect("x86 takes any start offset")
        .encode(&mut ours);
    let mut theirs = src.to_vec();
    assert!(sdk_encoder::x86_encode(&mut theirs, 0));
    // The converter leaves a tail it cannot see the end of alone; both stop
    // at the same place, so only the converted prefix is comparable.
    assert!(ours == theirs, "the x86 branch converter differs");
});

/// One comparison, with everything needed to reproduce it in the message.
#[expect(clippy::too_many_arguments, reason = "all of it is the repro")]
fn same(
    what: &str,
    props: &LzmaEncProps,
    mf_threads: u32,
    block_size: u64,
    block_threads: usize,
    src_len: usize,
    ours: &[u8],
    theirs: &[u8],
) {
    if ours == theirs {
        return;
    }
    let n = props.normalized();
    let at = ours
        .iter()
        .zip(theirs)
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| ours.len().min(theirs.len()));
    panic!(
        "{what} is not the SDK's: level={} bt={} nh={} lc={} lp={} pb={} fb={} dict={} \
         mf_threads={mf_threads} block={block_size} block_threads={block_threads} \
         input={src_len} bytes; ours {} bytes, the SDK's {}, first difference at {at}",
        n.level,
        n.bt_mode,
        n.num_hash_bytes,
        n.lc,
        n.lp,
        n.pb,
        n.fb,
        n.dict_size,
        ours.len(),
        theirs.len(),
    );
}
