//! Decode-throughput harness. See `docs/benchmarking.md`.
//!
//! ```text
//! lzma-bench [--runs N] [--no-oracles] <file.lzma|file.xz> ...
//! ```
//!
//! Decodes each input to a discard sink through the streaming API, reports
//! wall time and MiB/s of output, and prints a CRC32 of the output so parity
//! with the oracles can be checked rather than assumed. With oracles enabled
//! it times `7zz t -mmt=1`, `xz -dc -T1` and the reference C decoder
//! (`7lzma d`) on the same file in the same session and prints the ratio that
//! the acceptance gate is stated in.
//!
//! Containers: `.lzma` is the LZMA-alone format (13-byte header). `.xz` is
//! parsed just far enough to find a single LZMA2 block's filter property byte
//! and its compressed data; anything more elaborate (multi-block, filter
//! chains, `.7z` folders) is out of scope for a harness that only needs one
//! long single-threaded stream to time. `st.7z` is therefore not a bench
//! input: the LZMA2 lane uses `p256.bin.xz`, which is the same kind of single
//! LZMA2 stream in a container this tool can read without a 7z header parser.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use lzma_fast::{FinishMode, Lzma2Decoder, LzmaAloneHeader, LzmaDecoder, Status};

const OUT_CHUNK: usize = 1 << 20;
const REFERENCE_C_DECODER: &str = "supporting-codebases/7zip/C/Util/Lzma/_o/7lzma";

const HELP: &str = "\
usage: lzma-bench [--runs N] [--no-oracles] <file.lzma|file.xz> ...

  --runs N       repetitions per decoder (default 3); the median is reported
  --no-oracles   time only this crate, skip 7zz / xz / 7lzma
  --portable     force the portable decode loop instead of the assembly one";

fn main() {
    let mut runs = 3usize;
    let mut oracles = true;
    let mut portable = false;
    let mut files: Vec<PathBuf> = Vec::new();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runs" => {
                runs = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| fail("--runs needs a number"));
            }
            "--no-oracles" | "--only-ours" => oracles = false,
            "--portable" => portable = true,
            "-h" | "--help" => {
                println!("{HELP}");
                return;
            }
            other if other.starts_with('-') => fail(&format!("unknown option {other}")),
            other => files.push(PathBuf::from(other)),
        }
    }

    if files.is_empty() {
        println!("{HELP}");
        std::process::exit(2);
    }

    let loop_name = if portable || !lzma_fast::ASM_LOOP {
        "portable"
    } else {
        "asm"
    };
    println!(
        "lzma-bench (lzma-fast {}, {loop_name} loop), {runs} run(s), median",
        lzma_fast::VERSION
    );

    for file in &files {
        bench_one(file, runs, oracles, portable);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("lzma-bench: {msg}");
    std::process::exit(2);
}

struct Run {
    decode: Duration,
    total: Duration,
    bytes: u64,
    crc: u32,
}

fn bench_one(path: &Path, runs: usize, oracles: bool, portable: bool) {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("lzma-bench: {}: {e}", path.display());
            return;
        }
    };

    // The decoders are interleaved, one run of each per round, rather than
    // run in blocks. On a machine that is not idle - and this one rarely is -
    // a block schedule charges whichever decoder happened to run during a
    // busy stretch, and the ratio the acceptance gate is stated in is exactly
    // the quantity that distortion moves. Interleaving spreads any load over
    // all of them, and the median then compares like with like.
    let cmds = if oracles {
        oracle_commands(path)
    } else {
        Vec::new()
    };
    let mut ours: Vec<Run> = Vec::new();
    let mut oracle_times: Vec<Option<Vec<Duration>>> =
        cmds.iter().map(|_| Some(Vec::new())).collect();

    for _ in 0..runs {
        match decode_file(path, &data, portable) {
            Ok(r) => ours.push(r),
            Err(e) => {
                eprintln!("lzma-bench: {}: {e}", path.display());
                return;
            }
        }
        for (i, (_, cmd)) in cmds.iter().enumerate() {
            let Some(slot) = oracle_times[i].as_mut() else {
                continue;
            };
            match time_command(cmd) {
                Some(t) => slot.push(t),
                None => oracle_times[i] = None,
            }
        }
    }

    let bytes = ours[0].bytes;
    let crc = ours[0].crc;
    for r in &ours {
        assert_eq!(r.bytes, bytes, "decode is not deterministic in length");
        assert_eq!(r.crc, crc, "decode is not deterministic in content");
    }

    let ours_decode = median(ours.iter().map(|r| r.decode).collect());
    let ours_total = median(ours.iter().map(|r| r.total).collect());

    println!();
    println!(
        "{}  ({} packed -> {} decoded, crc32 {crc:08x})",
        path.display(),
        human(data.len() as u64),
        human(bytes)
    );
    println!("  {:<28} {:>9} {:>11}", "decoder", "time", "MiB/s");
    print_row("lzma-fast (decode only)", ours_decode, bytes);
    print_row("lzma-fast (incl. crc32)", ours_total, bytes);

    if !oracles {
        return;
    }

    let mut baseline: Option<Duration> = None;
    let mut c_baseline: Option<Duration> = None;
    for (i, (label, _)) in cmds.iter().enumerate() {
        let Some(times) = oracle_times[i].take() else {
            println!("  {label:<28} {:>9}", "n/a");
            continue;
        };
        let t = median(times);
        if label.starts_with("7zz") {
            baseline = Some(t);
        }
        if label.starts_with("7lzma") {
            c_baseline = Some(t);
        }
        print_row(label, t, bytes);
    }

    if let Some(b) = c_baseline {
        let r = ours_decode.as_secs_f64() / b.as_secs_f64();
        println!("  vs 7lzma (C, no asm):  {r:.4}");
    }
    if let Some(b) = baseline {
        let r = ours_decode.as_secs_f64() / b.as_secs_f64();
        println!(
            "  gate vs 7zz -mmt=1:    {r:.4}  ({})",
            if r <= 1.03 { "within 3%" } else { "OVER" }
        );
    }
}

fn print_row(label: &str, t: Duration, bytes: u64) {
    let secs = t.as_secs_f64();
    let mibs = (bytes as f64 / (1024.0 * 1024.0)) / secs;
    println!("  {label:<28} {secs:>8.3}s {mibs:>10.1}");
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort_unstable();
    v[v.len() / 2]
}

fn human(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut x = n as f64;
    let mut u = 0;
    while x >= 1024.0 && u + 1 < UNITS.len() {
        x /= 1024.0;
        u += 1;
    }
    format!("{x:.1} {}", UNITS[u])
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

fn decode_file(path: &Path, data: &[u8], portable: bool) -> Result<Run, String> {
    match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "lzma" => decode_lzma1(data, portable),
        "xz" => decode_lzma2(data, portable),
        ext => Err(format!("unsupported input extension {ext:?}")),
    }
}

fn decode_lzma1(data: &[u8], portable: bool) -> Result<Run, String> {
    if data.len() < 13 {
        return Err("truncated .lzma header".into());
    }
    let header: [u8; 13] = data[..13].try_into().unwrap();
    let header = LzmaAloneHeader::parse(&header).map_err(|e| e.to_string())?;
    let mut dec = if portable {
        LzmaDecoder::new_portable(header.props)
    } else {
        LzmaDecoder::new(header.props)
    }
    .map_err(|e| e.to_string())?;
    drive(&data[13..], &mut crc_sink(), |input, out| {
        dec.decode(input, out, FinishMode::Any)
            .map_err(|e| e.to_string())
    })
}

fn decode_lzma2(data: &[u8], portable: bool) -> Result<Run, String> {
    let (dict_prop, payload) = xz_lzma2_block(data).ok_or("not a single-block LZMA2 .xz")?;
    let mut dec = if portable {
        Lzma2Decoder::new_portable(dict_prop)
    } else {
        Lzma2Decoder::new(dict_prop)
    }
    .map_err(|e| e.to_string())?;
    drive(payload, &mut crc_sink(), |input, out| {
        dec.decode(input, out, FinishMode::Any)
            .map_err(|e| e.to_string())
    })
}

fn crc_sink() -> Crc32 {
    Crc32::new()
}

/// Runs the streaming loop, timing only the decoder calls so the CRC of the
/// output does not get charged to the decoder.
fn drive<F>(mut input: &[u8], crc: &mut Crc32, mut step: F) -> Result<Run, String>
where
    F: FnMut(&[u8], &mut [u8]) -> Result<lzma_fast::Progress, String>,
{
    let mut out = vec![0u8; OUT_CHUNK];
    let mut bytes = 0u64;
    let mut decode = Duration::ZERO;
    let started = Instant::now();

    loop {
        let t0 = Instant::now();
        let p = step(input, &mut out)?;
        decode += t0.elapsed();

        input = &input[p.read..];
        crc.update(&out[..p.written]);
        bytes += p.written as u64;

        if p.status == Status::FinishedWithMark || (p.read == 0 && p.written == 0) {
            break;
        }
    }

    Ok(Run {
        decode,
        total: started.elapsed(),
        bytes,
        crc: crc.finish(),
    })
}

/// Minimal single-block `.xz` reader; see the module docs for why it is this
/// small. Mirrors the test helper of the same name.
fn xz_lzma2_block(data: &[u8]) -> Option<(u8, &[u8])> {
    const MAGIC: &[u8] = &[0xFD, b'7', b'z', b'X', b'Z', 0x00];
    if data.len() < 12 || &data[..6] != MAGIC {
        return None;
    }
    let mut pos = 12;
    let first = *data.get(pos)?;
    if first == 0 {
        return None;
    }
    let header_end = pos + (usize::from(first) + 1) * 4;
    if header_end > data.len() {
        return None;
    }
    pos += 1;
    let flags = *data.get(pos)?;
    pos += 1;
    if flags & 0x3C != 0 || flags & 0x03 != 0 {
        return None;
    }
    if flags & 0x40 != 0 {
        xz_varint(data, &mut pos)?;
    }
    if flags & 0x80 != 0 {
        xz_varint(data, &mut pos)?;
    }
    let id = xz_varint(data, &mut pos)?;
    let props_size = xz_varint(data, &mut pos)?;
    if id != 0x21 || props_size != 1 {
        return None;
    }
    let dict_prop = *data.get(pos)?;
    Some((dict_prop, &data[header_end..]))
}

fn xz_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v: u64 = 0;
    for i in 0..9 {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= u64::from(b & 0x7F) << (i * 7);
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Oracles
// ---------------------------------------------------------------------------

fn oracle_commands(path: &Path) -> Vec<(String, Vec<String>)> {
    let p = path.display().to_string();
    let mut v = vec![(
        "7zz t -mmt=1 (asm loop)".to_string(),
        vec!["7zz".into(), "t".into(), "-mmt=1".into(), p.clone()],
    )];
    match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "lzma" => {
            v.push((
                "xz -dc -T1 --format=lzma".to_string(),
                vec![
                    "xz".into(),
                    "-dc".into(),
                    "-T1".into(),
                    "--format=lzma".into(),
                    p.clone(),
                ],
            ));
            if let Some(c) = reference_c_decoder() {
                v.push((
                    "7lzma d (C, no asm)".to_string(),
                    vec![c, "d".into(), p, "/dev/null".into()],
                ));
            }
        }
        "xz" => v.push((
            "xz -dc -T1".to_string(),
            vec!["xz".into(), "-dc".into(), "-T1".into(), p],
        )),
        _ => {}
    }
    v
}

/// The reference C decoder lives next to the 7-Zip checkout the port was made
/// from; the path is resolved from `$HOME` so no developer's home directory is
/// baked into the source.
fn reference_c_decoder() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let p = Path::new(&home).join("dev").join(REFERENCE_C_DECODER);
    p.exists().then(|| p.display().to_string())
}

fn time_command(cmd: &[String]) -> Option<Duration> {
    let t0 = Instant::now();
    let status = Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok()?;
    let dt = t0.elapsed();
    status.success().then_some(dt)
}

// ---------------------------------------------------------------------------
// CRC32 (IEEE), slicing-by-8. Only used outside the timed region.
// ---------------------------------------------------------------------------

struct Crc32 {
    state: u32,
    table: Box<[[u32; 256]; 8]>,
}

impl Crc32 {
    fn new() -> Self {
        let mut table = Box::new([[0u32; 256]; 8]);
        for i in 0..256usize {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            table[0][i] = c;
        }
        for i in 0..256usize {
            for k in 1..8 {
                let prev = table[k - 1][i];
                table[k][i] = (prev >> 8) ^ table[0][(prev & 0xFF) as usize];
            }
        }
        Crc32 {
            state: 0xFFFF_FFFF,
            table,
        }
    }

    fn update(&mut self, buf: &[u8]) {
        let mut crc = self.state;
        let mut chunks = buf.chunks_exact(8);
        for c in &mut chunks {
            let lo = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) ^ crc;
            let hi = u32::from_le_bytes([c[4], c[5], c[6], c[7]]);
            crc = self.table[7][(lo & 0xFF) as usize]
                ^ self.table[6][((lo >> 8) & 0xFF) as usize]
                ^ self.table[5][((lo >> 16) & 0xFF) as usize]
                ^ self.table[4][(lo >> 24) as usize]
                ^ self.table[3][(hi & 0xFF) as usize]
                ^ self.table[2][((hi >> 8) & 0xFF) as usize]
                ^ self.table[1][((hi >> 16) & 0xFF) as usize]
                ^ self.table[0][(hi >> 24) as usize];
        }
        for &b in chunks.remainder() {
            crc = (crc >> 8) ^ self.table[0][((crc ^ u32::from(b)) & 0xFF) as usize];
        }
        self.state = crc;
    }

    fn finish(&self) -> u32 {
        self.state ^ 0xFFFF_FFFF
    }
}
