//! The `.xz` container lane: whole files, not one LZMA2 stream.
//!
//! What the other lanes time is the LZMA2 decoder with the container peeled
//! off. This one times what a consumer actually calls - `XzReader` and
//! `XzParallelReader` over a whole `.xz` file, headers, filters, checks and
//! index included - against the tools such a consumer would otherwise use:
//! `xz -dc -T<n>`, `7zz t`, and the `liblzma` crate, which is the C library
//! this crate exists to replace.

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use lzma_turbo::xz::{XzOptions, XzParallelReader, XzReader};

use crate::{
    CrcWriter, MtRun, OUT_CHUNK, alloc_watch_peak, alloc_watch_reset, human, median, time_command,
};

/// Times one `.xz` file: the sequential reader, then the parallel reader at
/// each requested thread count.
pub fn bench(path: &Path, runs: usize, oracles: bool, threads: &[usize]) {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("lzma-bench: {}: {e}", path.display());
            return;
        }
    };
    let blocks = {
        let mut c = std::io::Cursor::new(&data);
        lzma_turbo::xz::single_stream_block_count(&mut c)
    };

    println!();
    println!(
        "{}  ({} packed, {})",
        path.display(),
        human(data.len() as u64),
        match blocks {
            Some(n) => format!("{n} block(s), one stream"),
            None => "several streams".to_string(),
        }
    );

    // -- sequential --------------------------------------------------------
    let mut ours: Vec<MtRun> = Vec::new();
    let mut lib: Option<Vec<Duration>> = oracles.then(Vec::new);
    let mut xz1: Option<Vec<Duration>> = oracles.then(Vec::new);
    let mut z7: Option<Vec<Duration>> = oracles.then(Vec::new);
    for _ in 0..runs {
        match seq_decode(&data) {
            Ok(r) => ours.push(r),
            Err(e) => {
                eprintln!("lzma-bench: {}: {e}", path.display());
                return;
            }
        }
        if let Some(slot) = lib.as_mut() {
            match liblzma_decode(&data, 1) {
                Ok(r) => slot.push(r.time),
                Err(e) => {
                    eprintln!("lzma-bench: liblzma: {e}");
                    lib = None;
                }
            }
        }
        if let Some(slot) = xz1.as_mut() {
            match time_command(&cmd_xz(path, 1)) {
                Some(d) => slot.push(d),
                None => xz1 = None,
            }
        }
        if let Some(slot) = z7.as_mut() {
            match time_command(&[
                "7zz".to_string(),
                "t".into(),
                "-mmt=1".into(),
                path.display().to_string(),
            ]) {
                Some(d) => slot.push(d),
                None => z7 = None,
            }
        }
    }
    let bytes = ours[0].bytes;
    let crc = ours[0].crc;
    for r in &ours {
        assert_eq!((r.bytes, r.crc), (bytes, crc), "the decode is not stable");
    }
    let seq = median(ours.iter().map(|r| r.time).collect());
    println!("  {} decoded, crc32 {crc:08x}", human(bytes));
    println!("  {:<28} {:>9} {:>11}", "sequential", "time", "MiB/s");
    row("XzReader", seq, bytes);
    if let Some(t) = xz1.map(median) {
        row("xz -dc -T1", t, bytes);
        let r = seq.as_secs_f64() / t.as_secs_f64();
        println!(
            "  gate vs xz -dc -T1:    {r:.4}  ({})",
            if r <= 1.03 { "within 3%" } else { "OVER" }
        );
    }
    if let Some(t) = z7.map(median) {
        row("7zz t -mmt=1", t, bytes);
    }
    if let Some(t) = lib.map(median) {
        row("liblzma (crate), 1 thread", t, bytes);
    }

    if threads.is_empty() {
        return;
    }

    // -- parallel ----------------------------------------------------------
    println!();
    println!(
        "  {:>7} {:>9} {:>10} {:>10} {:>9} {:>9} {:>9}",
        "threads", "ours", "MiB/s", "peak RAM", "xz -T", "liblzma", "ratios"
    );
    for &t in threads {
        let mut ours: Vec<MtRun> = Vec::new();
        let mut lib: Option<Vec<Duration>> = oracles.then(Vec::new);
        let mut lib_peak = 0u64;
        let mut xzn: Option<Vec<Duration>> = oracles.then(Vec::new);
        for _ in 0..runs {
            match par_decode(&data, t) {
                Ok(r) => ours.push(r),
                Err(e) => {
                    eprintln!("lzma-bench: {} at {t} threads: {e}", path.display());
                    return;
                }
            }
            if let Some(slot) = lib.as_mut() {
                match liblzma_decode(&data, t) {
                    Ok(r) => {
                        lib_peak = lib_peak.max(r.peak);
                        slot.push(r.time);
                    }
                    Err(e) => {
                        eprintln!("lzma-bench: liblzma at {t} threads: {e}");
                        lib = None;
                    }
                }
            }
            if let Some(slot) = xzn.as_mut() {
                match time_command(&cmd_xz(path, t)) {
                    Some(d) => slot.push(d),
                    None => xzn = None,
                }
            }
        }
        for r in &ours {
            assert_eq!(
                (r.bytes, r.crc),
                (bytes, crc),
                "the parallel decode disagreed with the sequential one"
            );
        }
        let t_ours = median(ours.iter().map(|r| r.time).collect());
        let peak = ours.iter().map(|r| r.peak).max().unwrap_or(0);
        let t_xz = xzn.map(median);
        let t_lib = lib.map(median);
        let secs = t_ours.as_secs_f64();
        let mibs = (bytes as f64 / (1024.0 * 1024.0)) / secs;
        let mut ratios = String::new();
        if let Some(b) = t_xz {
            ratios.push_str(&format!("xz {:.3}", secs / b.as_secs_f64()));
        }
        if let Some(b) = t_lib {
            if !ratios.is_empty() {
                ratios.push_str(", ");
            }
            ratios.push_str(&format!("liblzma {:.3}", secs / b.as_secs_f64()));
        }
        println!(
            "  {t:>7} {secs:>8.3}s {mibs:>10.1} {:>10} {:>9} {:>9}  {ratios}",
            human(peak),
            t_xz.map_or("n/a".to_string(), |d| format!("{:.3}s", d.as_secs_f64())),
            t_lib.map_or("n/a".to_string(), |d| format!("{:.3}s", d.as_secs_f64())),
        );
        if lib_peak > 0 {
            println!("  {:>7} {:>50} {}", "", "liblzma peak RAM", human(lib_peak));
        }
    }
}

fn row(label: &str, t: Duration, bytes: u64) {
    let secs = t.as_secs_f64();
    let mibs = (bytes as f64 / (1024.0 * 1024.0)) / secs;
    println!("  {label:<28} {secs:>8.3}s {mibs:>10.1}");
}

fn cmd_xz(path: &Path, threads: usize) -> Vec<String> {
    vec![
        "xz".into(),
        "-dc".into(),
        format!("-T{threads}"),
        path.display().to_string(),
    ]
}

fn seq_decode(data: &[u8]) -> Result<MtRun, String> {
    let mut sink = CrcWriter::new();
    let mut buf = vec![0u8; OUT_CHUNK];
    let base = alloc_watch_reset();
    let t0 = Instant::now();
    let mut r = XzReader::new(data);
    let mut bytes = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        sink.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    Ok(MtRun {
        time: t0.elapsed(),
        peak: alloc_watch_peak(base),
        bytes,
        crc: sink.crc.finish(),
    })
}

fn par_decode(data: &[u8], threads: usize) -> Result<MtRun, String> {
    let mut sink = CrcWriter::new();
    let mut buf = vec![0u8; OUT_CHUNK];
    let base = alloc_watch_reset();
    let t0 = Instant::now();
    let opts = XzOptions::default().with_threads(threads);
    let mut r = XzParallelReader::with_options(std::io::Cursor::new(data), opts)
        .map_err(|e| e.to_string())?;
    let mut bytes = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        sink.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    Ok(MtRun {
        time: t0.elapsed(),
        peak: alloc_watch_peak(base),
        bytes,
        crc: sink.crc.finish(),
    })
}

/// The C library, through the `liblzma` crate, in the same shape weaver calls
/// it: a multi-stream decoder, multi-threaded when asked.
fn liblzma_decode(data: &[u8], threads: usize) -> Result<MtRun, String> {
    let mut sink = CrcWriter::new();
    let mut buf = vec![0u8; OUT_CHUNK];
    let base = alloc_watch_reset();
    let t0 = Instant::now();
    let mut r: Box<dyn Read> = if threads > 1 {
        // The same builder weaver uses: `lzma_stream_decoder_mt`, with
        // concatenated streams allowed and no memory ceiling, so the C is
        // asked for exactly what this crate is asked for.
        let stream = liblzma::stream::MtStreamBuilder::new()
            .threads(u32::try_from(threads).map_err(|e| e.to_string())?)
            .memlimit_stop(u64::MAX)
            .memlimit_threading(u64::MAX)
            .decoder()
            .map_err(|e| e.to_string())?;
        Box::new(liblzma::read::XzDecoder::new_stream(data, stream))
    } else {
        Box::new(liblzma::read::XzDecoder::new_multi_decoder(data))
    };
    let mut bytes = 0u64;
    loop {
        let n = r.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        sink.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    Ok(MtRun {
        time: t0.elapsed(),
        peak: alloc_watch_peak(base),
        bytes,
        crc: sink.crc.finish(),
    })
}
