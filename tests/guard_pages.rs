//! Both decode loops with every buffer against a no-access page.
//!
//! A sanitizer cannot see inside the assembly loop, and a stray read a few
//! bytes past a heap buffer usually lands in memory the process owns, so an
//! overrun can pass every differential test. This binary's allocator puts
//! each allocation flush against a page that faults on any access — after it
//! for the first pass, before it for the second — and the driver copies every
//! input slice it feeds into an allocation of exactly that size. The
//! dictionary, the probability table and the input then each end (and begin)
//! on a guard, so a read or write one byte outside any of them kills the test
//! with a fault instead of going unseen. `a_read_past_an_allocation_faults`
//! proves the guard is really there.

#![cfg(all(target_pointer_width = "64", any(unix, windows)))]

mod common;

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicBool, Ordering},
};

use common::*;
use lzma_turbo::{FinishMode, Lzma2Decoder, LzmaAloneHeader, LzmaDecoder, Status};

/// The guard and rounding unit: a multiple of every page size in use (4 KiB,
/// 16 KiB) and Windows' allocation granularity.
const UNIT: usize = 64 * 1024;

/// Place allocations against the guard before them rather than after.
static FRONT: AtomicBool = AtomicBool::new(false);

struct Guarded;

#[global_allocator]
static ALLOCATOR: Guarded = Guarded;

// Each allocation is [guard][data, rounded up to UNIT][guard]; the returned
// pointer sits at the start of the data region (FRONT) or ends flush with its
// end (otherwise, give or take less than `align` bytes of slack). Either way
// it lies in the region's first UNIT bytes, which is how `dealloc` finds the
// region again without knowing which way it was placed.
unsafe impl GlobalAlloc for Guarded {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > UNIT {
            return unsafe { System.alloc(layout) };
        }
        let data = layout.size().div_ceil(UNIT).max(1) * UNIT;
        let Some(base) = os::map(UNIT + data + UNIT, UNIT, data) else {
            return std::ptr::null_mut();
        };
        // `dealloc` rounds down to UNIT to find `base`; anything else would
        // unmap a neighbour's pages.
        assert_eq!(base as usize % UNIT, 0, "guarded region not UNIT-aligned");
        let start = base as usize + UNIT;
        let ptr = if FRONT.load(Ordering::Relaxed) {
            start
        } else {
            (start + data - layout.size()) & !(layout.align() - 1)
        };
        ptr as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.align() > UNIT {
            return unsafe { System.dealloc(ptr, layout) };
        }
        let data = layout.size().div_ceil(UNIT).max(1) * UNIT;
        let base = (ptr as usize & !(UNIT - 1)) - UNIT;
        os::unmap(base as *mut u8, UNIT + data + UNIT);
    }
}

#[cfg(unix)]
mod os {
    use std::ffi::{c_int, c_void};

    unsafe extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        fn mprotect(addr: *mut c_void, len: usize, prot: c_int) -> c_int;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
    }

    const PROT_NONE: c_int = 0;
    const PROT_READ_WRITE: c_int = 1 | 2;
    const MAP_PRIVATE: c_int = 2;
    #[cfg(target_vendor = "apple")]
    const MAP_ANON: c_int = 0x1000;
    #[cfg(not(target_vendor = "apple"))]
    const MAP_ANON: c_int = 0x20;

    /// Reserves `total` bytes with no access, starting on a `UNIT` boundary,
    /// and opens `len` of them at `offset` for reading and writing.
    ///
    /// `mmap` only promises page alignment, and `dealloc` finds the region's
    /// base by rounding down to `UNIT`, so the reservation is over-sized by a
    /// `UNIT` and trimmed to an aligned `total` bytes.
    pub fn map(total: usize, offset: usize, len: usize) -> Option<*mut u8> {
        let span = total + super::UNIT;
        // SAFETY: an anonymous private mapping at an address of the kernel's
        // choosing, the unmapping of its unaligned ends, then a protection
        // change inside what is left.
        unsafe {
            let raw = mmap(
                std::ptr::null_mut(),
                span,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANON,
                -1,
                0,
            );
            if raw as isize == -1 {
                return None;
            }
            let raw = raw.cast::<u8>();
            let head = (raw as usize).next_multiple_of(super::UNIT) - raw as usize;
            let base = raw.add(head);
            if head > 0 {
                munmap(raw.cast(), head);
            }
            let tail = span - head - total;
            if tail > 0 {
                munmap(base.add(total).cast(), tail);
            }
            if mprotect(base.add(offset).cast(), len, PROT_READ_WRITE) != 0 {
                munmap(base.cast(), total);
                return None;
            }
            Some(base)
        }
    }

    pub fn unmap(base: *mut u8, total: usize) {
        // SAFETY: `base` and `total` are what `map` mapped.
        unsafe { munmap(base.cast(), total) };
    }
}

#[cfg(windows)]
mod os {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualAlloc(addr: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
        fn VirtualFree(addr: *mut c_void, size: usize, kind: u32) -> i32;
    }

    const MEM_COMMIT: u32 = 0x1000;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_READWRITE: u32 = 0x04;

    /// Reserves `total` bytes with no access and commits `len` of them at
    /// `offset` for reading and writing.
    pub fn map(total: usize, offset: usize, len: usize) -> Option<*mut u8> {
        // SAFETY: a fresh reservation, then a commit inside it.
        unsafe {
            let base = VirtualAlloc(std::ptr::null_mut(), total, MEM_RESERVE, PAGE_NOACCESS);
            if base.is_null() {
                return None;
            }
            let data = base.cast::<u8>().add(offset).cast();
            if VirtualAlloc(data, len, MEM_COMMIT, PAGE_READWRITE).is_null() {
                VirtualFree(base, 0, MEM_RELEASE);
                return None;
            }
            Some(base.cast())
        }
    }

    pub fn unmap(base: *mut u8, _total: usize) {
        // SAFETY: `base` is what `map` reserved.
        unsafe { VirtualFree(base.cast(), 0, MEM_RELEASE) };
    }
}

#[test]
fn a_read_past_an_allocation_faults() {
    // In a child process, so that the fault is observed rather than suffered.
    const CHILD: &str = "LZMA_TURBO_GUARD_SELF_TEST";
    if let Some(side) = std::env::var_os(CHILD) {
        FRONT.store(side == "front", Ordering::Relaxed);
        let buffer = vec![7u8; 1000];
        let outside = if side == "front" {
            buffer.as_ptr().wrapping_sub(1)
        } else {
            buffer.as_ptr().wrapping_add(buffer.len())
        };
        // SAFETY: none; this is the out-of-bounds read the guard must stop.
        let byte = unsafe { outside.read_volatile() };
        println!("read {byte} outside the buffer");
        return;
    }
    for side in ["back", "front"] {
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "a_read_past_an_allocation_faults",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, side)
            .output()
            .expect("run the child");
        assert!(
            faulted(status.status),
            "a read one byte {side} of an allocation did not fault ({:?}):\n{}",
            status.status,
            String::from_utf8_lossy(&status.stdout)
        );
    }
}

/// Killed by an access violation, not failed some other way.
#[cfg(unix)]
fn faulted(status: std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    // SIGSEGV everywhere; SIGBUS is 10 on macOS and 7 on Linux.
    matches!(status.signal(), Some(11 | 10 | 7))
}

#[cfg(windows)]
fn faulted(status: std::process::ExitStatus) -> bool {
    // STATUS_ACCESS_VIOLATION.
    status.code() == Some(0xC000_0005_u32 as i32)
}

/// Input and output slice sizes, as `asm_parity.rs`, less the one-byte
/// output lane: every call makes two fresh guarded allocations, and a
/// one-byte output makes one call per byte.
const CHUNKS: &[(usize, usize)] = &[(1, 4096), (7, 13), (4096, 4096), (usize::MAX, 1 << 20)];

/// `decode_lzma_with`, but every input slice is copied into an allocation of
/// exactly its length, so the byte after it is a guard.
fn lzma_guarded(data: &[u8], in_chunk: usize, out_chunk: usize, portable: bool) -> Option<Vec<u8>> {
    let header: [u8; 13] = data.get(..13)?.try_into().ok()?;
    let header = LzmaAloneHeader::parse(&header).ok()?;
    let mut dec = if portable {
        LzmaDecoder::new_portable(header.props).ok()?
    } else {
        LzmaDecoder::new(header.props).ok()?
    };
    let mut input = &data[13..];
    let mut out = Vec::new();
    let mut remaining = header.uncompressed_size;
    loop {
        let limit = remaining.map_or(out_chunk, |r| out_chunk.min(r as usize));
        if limit == 0 {
            break;
        }
        let finish = match remaining {
            Some(r) if (r as usize) <= limit => FinishMode::End,
            _ => FinishMode::Any,
        };
        let feed = input[..in_chunk.min(input.len())].to_vec();
        let mut buf = vec![0u8; limit];
        let p = dec.decode(&feed, &mut buf, finish).ok()?;
        input = &input[p.read..];
        out.extend_from_slice(&buf[..p.written]);
        if let Some(r) = remaining.as_mut() {
            *r -= p.written as u64;
        }
        if p.status == Status::FinishedWithMark || (p.read == 0 && p.written == 0) {
            break;
        }
    }
    Some(out)
}

fn lzma2_guarded(
    prop: u8,
    data: &[u8],
    in_chunk: usize,
    out_chunk: usize,
    portable: bool,
) -> Option<Vec<u8>> {
    let mut dec = if portable {
        Lzma2Decoder::new_portable(prop).ok()?
    } else {
        Lzma2Decoder::new(prop).ok()?
    };
    let mut input = data;
    let mut out = Vec::new();
    loop {
        let feed = input[..in_chunk.min(input.len())].to_vec();
        let mut buf = vec![0u8; out_chunk];
        let p = dec.decode(&feed, &mut buf, FinishMode::Any).ok()?;
        input = &input[p.read..];
        out.extend_from_slice(&buf[..p.written]);
        if p.status == Status::FinishedWithMark || (p.read == 0 && p.written == 0) {
            break;
        }
    }
    Some(out)
}

/// Every stream the pass decodes: the vectors in each slice shape, every
/// truncation of the first 512 bytes of each (where the input ends inside
/// the range coder's first symbols), and bit-flipped copies.
fn one_pass() {
    for portable in [false, true] {
        for stem in SOURCES {
            for variant in LZMA_VARIANTS {
                let name = format!("{stem}.{variant}.lzma");
                let data = read(&name);
                for &(i, o) in CHUNKS {
                    let out = lzma_guarded(&data, i, o, portable);
                    assert!(out.is_some(), "{name} ({i},{o}) portable={portable}");
                }
                for cut in 13..data.len().min(512) {
                    let _ = lzma_guarded(&data[..cut], usize::MAX, 1 << 16, portable);
                }
                let mut rng = 0x6A09_E667_F3BC_C908u64 ^ data.len() as u64;
                for _ in 0..40 {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    if data.len() > 13 {
                        let mut bad = data.clone();
                        let at = 13 + (rng as usize >> 8) % (bad.len() - 13);
                        bad[at] ^= 1 << (rng % 8);
                        let _ = lzma_guarded(&bad, 7, 4096, portable);
                    }
                }
            }
            for variant in XZ_VARIANTS {
                let name = format!("{stem}.{variant}.xz");
                let raw = read(&name);
                let Some((prop, block)) = xz_lzma2_block(&raw) else {
                    continue;
                };
                for &(i, o) in CHUNKS {
                    let out = lzma2_guarded(prop, block, i, o, portable);
                    assert!(out.is_some(), "{name} ({i},{o}) portable={portable}");
                }
                for cut in 0..block.len().min(512) {
                    let _ = lzma2_guarded(prop, &block[..cut], usize::MAX, 1 << 16, portable);
                }
            }
        }
    }
}

#[test]
#[ignore = "minutes on Windows; CI's memory-safety job runs it with --include-ignored"]
fn both_loops_stay_inside_every_buffer() {
    FRONT.store(false, Ordering::Relaxed);
    one_pass();
    FRONT.store(true, Ordering::Relaxed);
    one_pass();
    FRONT.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// The encoder.
// ---------------------------------------------------------------------------
//
// The decode side above is driven through slices the caller owns; the encode
// side owns almost everything it touches — the match finder's window, its hash
// and `son` tables, the probability arrays, the optimal parser's `opt` array
// and the range encoder's 64 KiB buffer — and the C it is ported from indexes
// several of those past their logical end on purpose (`kMatchSpecLenStart`
// sentinels, `MatchFinder_Normalize3`'s pass over the whole table,
// `GetMatchesSpecN_2`'s `cur + 4` reads). Under this allocator every one of
// those ends flush against a page that faults, so an index one element too far
// is a fault rather than a silently wrong byte, and the threaded finder's
// three threads are watched over the same memory at once.

#[cfg(feature = "enc")]
mod encoder {
    use std::sync::atomic::Ordering;

    use lzma_turbo::{
        Error, Lzma2Encoder, LzmaEncProps, LzmaEncoder, MatchFinderKind, SeqOutStream, SliceStream,
    };

    use super::FRONT;

    /// A sink of exactly the length the encode is known to produce, so its
    /// last byte is the last byte before a guard.
    ///
    /// A `Vec` that grows cannot do this: its buffer is a power-of-two
    /// capacity with slack at the end, and the slack is what an overrun lands
    /// in. Every case below therefore encodes twice — once into a `Vec` to
    /// learn the length, once into this — and requires the two to agree.
    struct ExactSink {
        buf: Vec<u8>,
        at: usize,
    }

    impl ExactSink {
        fn new(len: usize) -> Self {
            let mut buf = Vec::with_capacity(len);
            buf.resize(len, 0);
            ExactSink { buf, at: 0 }
        }
    }

    impl SeqOutStream for ExactSink {
        fn write(&mut self, data: &[u8]) -> Result<(), Error> {
            let end = self.at + data.len();
            assert!(end <= self.buf.len(), "the encoder wrote more than it did");
            self.buf[self.at..end].copy_from_slice(data);
            self.at = end;
            Ok(())
        }
    }

    /// `src` copied into an allocation of exactly its length: the byte after
    /// it is a guard, so a match finder that reads one past the input it was
    /// given faults here.
    fn exact(src: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(src.len());
        v.extend_from_slice(src);
        v
    }

    /// Shapes the parser and the match finder take different branches on,
    /// kept small: every allocation here costs three pages of address space
    /// and the point is the edges, not the volume.
    fn inputs() -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<(String, Vec<u8>)> = vec![
            ("empty".into(), Vec::new()),
            ("one".into(), vec![0x2A]),
            ("zeros".into(), vec![0; 70_000]),
        ];
        // A run-and-phrase mix, which is what gives the binary tree long
        // chains, and incompressible bytes, which is what makes the copy-chunk
        // fallback run.
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut mixed = Vec::with_capacity(80_000);
        while mixed.len() < 80_000 {
            let n = (next() >> 40) as usize % 60 + 1;
            let b = (next() >> 32) as u8;
            if next() & 1 == 0 {
                mixed.extend(std::iter::repeat_n(b, n));
            } else {
                mixed.extend((0..n).map(|_| next() as u8));
            }
        }
        out.push(("mixed".into(), mixed));
        out.push((
            "random".into(),
            (0..70_000).map(|_| next() as u8).collect::<Vec<u8>>(),
        ));
        out
    }

    /// Small enough that the window slides and the hash normalizes over these
    /// inputs, and large enough for the other one to stay solid.
    const DICTS: [u32; 2] = [1 << 16, 1 << 20];

    fn lzma1_pass() {
        for (name, data) in inputs() {
            let src = exact(&data);
            for kind in [
                MatchFinderKind::Hc4,
                MatchFinderKind::Bt2,
                MatchFinderKind::Bt4,
                MatchFinderKind::Bt5,
            ] {
                for dict in DICTS {
                    for level in [1u32, 9] {
                        let props = LzmaEncProps::new()
                            .with_level(level)
                            .with_match_finder(kind)
                            .with_dict_size(dict);
                        let what = format!("{name} {kind:?} dict={dict} level={level}");
                        let reference = LzmaEncoder::new(&props)
                            .expect("encoder")
                            .encode_to_vec(&src)
                            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                        let mut sink = ExactSink::new(reference.len());
                        let mut enc = LzmaEncoder::new(&props).expect("encoder");
                        let mut input = SliceStream::new(&src);
                        enc.encode_sized(&mut input, &mut sink, src.len() as u64)
                            .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                        assert!(sink.buf == reference, "{what}: exact sink differs");
                    }
                }
            }
        }
    }

    fn lzma2_pass() {
        for (name, data) in inputs() {
            let src = exact(&data);
            for dict in DICTS {
                // Solid on one thread, then the same input cut into blocks and
                // encoded on four: the block coder's own buffers, the free
                // block list and the write turn, all against guards.
                for (threads, block) in [(1usize, 0u64), (4, 16 * 1024), (4, 1 << 16)] {
                    let props = LzmaEncProps::new().with_level(5).with_dict_size(dict);
                    let what = format!("{name} dict={dict} threads={threads} block={block}");
                    let mut enc = Lzma2Encoder::new(&props).expect("encoder");
                    if block != 0 {
                        enc.set_block_size(block);
                        enc.set_threads(threads);
                    }
                    enc.set_data_size(src.len() as u64);
                    let reference = enc
                        .encode_to_vec(&src)
                        .unwrap_or_else(|e| panic!("{what}: {e:?}"));

                    let mut enc = Lzma2Encoder::new(&props).expect("encoder");
                    if block != 0 {
                        enc.set_block_size(block);
                        enc.set_threads(threads);
                    }
                    enc.set_data_size(src.len() as u64);
                    let mut sink = ExactSink::new(reference.len());
                    enc.encode_slice(&src, &mut sink)
                        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                    assert!(sink.buf == reference, "{what}: exact sink differs");
                }
            }
        }
    }

    /// `LzFindMt.c`: the hash thread, the bt thread and the caller all reading
    /// and writing one window and one pair of ring buffers. Every one of those
    /// is a guarded allocation here, so a block index that runs off the end of
    /// `hashBuf` or `btBuf` faults on whichever thread did it.
    fn mt_finder_pass() {
        for (name, data) in inputs() {
            let src = exact(&data);
            for dict in DICTS {
                for kind in [MatchFinderKind::Bt4, MatchFinderKind::Bt5] {
                    let props = LzmaEncProps::new()
                        .with_level(9)
                        .with_match_finder(kind)
                        .with_dict_size(dict)
                        .with_num_threads(2);
                    let what = format!("{name} {kind:?} dict={dict} mf-threads=2");
                    let mut enc = Lzma2Encoder::new(&props).expect("encoder");
                    enc.set_data_size(src.len() as u64);
                    let reference = enc
                        .encode_to_vec(&src)
                        .unwrap_or_else(|e| panic!("{what}: {e:?}"));

                    let mut enc = Lzma2Encoder::new(&props).expect("encoder");
                    enc.set_data_size(src.len() as u64);
                    let mut sink = ExactSink::new(reference.len());
                    let mut input = SliceStream::new(&src);
                    enc.encode_send(&mut input, &mut sink)
                        .unwrap_or_else(|e| panic!("{what}: {e:?}"));
                    assert!(sink.buf == reference, "{what}: exact sink differs");
                }
            }
        }
    }

    #[test]
    #[ignore = "minutes; CI's memory-safety job runs it with --include-ignored"]
    fn the_encoders_stay_inside_every_buffer() {
        for front in [false, true] {
            FRONT.store(front, Ordering::Relaxed);
            lzma1_pass();
            lzma2_pass();
            mt_finder_pass();
        }
        FRONT.store(false, Ordering::Relaxed);
    }
}
