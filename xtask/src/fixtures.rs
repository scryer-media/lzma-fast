//! `cargo xtask fixtures`: generates the benchmark fixtures under
//! `bench/fixtures`. They are gitignored and never committed. The payload is
//! seeded, so it is the same bytes on every machine.
//!
//! Requires `xz` (XZ Utils), `7zz` (7-Zip) and `tar` on `PATH`, and about
//! 4.5 GiB free. A fixture that already exists is left alone.

use std::{
    env,
    fs::{self, File},
    io::{self, BufWriter, Read, Write},
    path::Path,
    process::{Command, ExitCode, Stdio},
};

use crate::{
    cmd::{repo_root, run, run_to_file, which},
    rng::Rng,
};

const MIB: usize = 1 << 20;

pub fn fixtures() -> ExitCode {
    let root = repo_root();
    let out = root.join("bench").join("fixtures");
    fs::create_dir_all(&out).expect("create bench/fixtures");
    env::set_current_dir(&out).expect("enter bench/fixtures");

    for tool in ["xz", "7zz", "tar"] {
        if which(tool).is_none() {
            eprintln!("fixtures: `{tool}` is not on PATH");
            return ExitCode::FAILURE;
        }
    }

    let mut ok = true;
    let mut make = |name: &str, build: &dyn Fn(&Path) -> bool| {
        let path = Path::new(name);
        if path.exists() {
            return;
        }
        println!("fixtures: {name}");
        if !build(path) {
            eprintln!("fixtures: could not build {name}");
            let _ = fs::remove_file(path);
            ok = false;
        }
    };
    make("payload.bin", &|path| write_payload(path, 1024).is_ok());
    make("p256.bin", &|path| {
        head("payload.bin".as_ref(), path, 256 * MIB).is_ok()
    });

    // LZMA1 (LZMA-alone container, 13-byte header) at xz preset 5, one thread.
    make(
        "p256.bin.lzma",
        &xz(&["-T1", "-5", "--format=lzma"], "p256.bin"),
    );
    make(
        "payload.bin.lzma",
        &xz(&["-T1", "-5", "--format=lzma"], "payload.bin"),
    );

    // LZMA2 inside xz, preset 5, single stream.
    make("p256.bin.xz", &xz(&["-T1", "-5"], "p256.bin"));

    // LZMA2 inside 7z: st = one stream, mt = 7-Zip's multi-threaded chunking.
    for (name, threads) in [("st.7z", "-mmt=1"), ("mt.7z", "-mmt=on")] {
        make(name, &|_| {
            run(
                "7zz",
                &[
                    "a",
                    "-bso0",
                    "-bsp0",
                    "-mx=5",
                    "-m0=lzma2",
                    threads,
                    name,
                    "payload.bin",
                ],
            )
        });
    }

    // --- the .xz container lane ---------------------------------------------
    // The fixtures the `--xz` mode of lzma-bench times: whole files with real
    // block layouts, filters and checks, rather than one long LZMA2 stream.

    // Multi-block, which is the only shape a parallel decode can help with.
    // `-T8` picks the block size from the thread count; the 16 MiB one fixes
    // it, so the two rows say what block size costs independently of how many
    // there are.
    make("p256.t8.xz", &xz(&["-T8", "-5"], "p256.bin"));
    make(
        "p256.b16.xz",
        &xz(&["-T8", "-5", "--block-size=16MiB"], "p256.bin"),
    );
    make("payload.t8.xz", &xz(&["-T8", "-5"], "payload.bin"));

    // The filters, so the converter pipeline is timed and not just LZMA2. The
    // x86 filter wants something with machine code in it; the local `xz`
    // binary is the one executable every machine running this has.
    let xz_binary = which("xz").expect("checked above");
    let xz_binary = xz_binary.to_string_lossy();
    make(
        "bcj-x86.xz",
        &xz(&["-T1", "-5", "--x86", "--lzma2=preset=5"], &xz_binary),
    );
    make(
        "delta.xz",
        &xz(
            &["-T1", "-5", "--delta=dist=4", "--lzma2=preset=5"],
            "p256.bin",
        ),
    );

    // The checks. CRC-64 is the default and is covered by the rows above.
    make(
        "p256.sha256.xz",
        &xz(&["-T1", "-5", "--check=sha256"], "p256.bin"),
    );
    make(
        "p256.crc32.xz",
        &xz(&["-T1", "-5", "--check=crc32"], "p256.bin"),
    );

    // Several streams in one file, which `xz -d` accepts by default.
    make("multi.xz", &|path| {
        concat(&["p256.bin.xz"; 3], path).is_ok()
    });

    // And a real-world shape: a tarball, which is what an .xz usually is.
    make("tree.tar.xz", &|path| tar_xz(&root, path).is_ok());

    if let Ok(entries) = fs::read_dir(&out) {
        let mut rows: Vec<_> = entries
            .flatten()
            .filter_map(|e| Some((e.file_name(), e.metadata().ok()?.len())))
            .collect();
        rows.sort();
        for (name, len) in rows {
            println!("{len:>12}  {}", name.to_string_lossy());
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `xz <args> -c <input> > path`
fn xz<'a>(args: &[&'a str], input: &'a str) -> impl Fn(&Path) -> bool + 'a {
    let mut all = args.to_vec();
    all.extend(["-c", input]);
    move |path| run_to_file("xz", &all, path)
}

/// `mib` MiB (1 GiB for the fixture), semi-compressible: a vocabulary of hex words mixed with runs of
/// random bytes, in 1 MiB chunks.
fn write_payload(path: &Path, mib: usize) -> io::Result<()> {
    let mut rng = Rng::new(7);
    let words: Vec<Vec<u8>> = (0..4000)
        .map(|_| {
            let mut raw = Vec::new();
            let len = rng.range(3, 9);
            rng.fill(&mut raw, len);
            raw.iter()
                .flat_map(|byte| format!("{byte:02x}").into_bytes())
                .collect()
        })
        .collect();

    let mut file = BufWriter::new(File::create(path)?);
    let mut chunk = Vec::with_capacity(MIB + 512);
    for _ in 0..mib {
        chunk.clear();
        while chunk.len() < MIB {
            if rng.chance(70) {
                let word = rng.range(0, words.len() - 1);
                chunk.extend_from_slice(&words[word]);
                chunk.push(b' ');
            } else {
                let len = rng.range(16, 256);
                rng.fill(&mut chunk, len);
            }
        }
        file.write_all(&chunk[..MIB])?;
    }
    file.flush()
}

fn head(from: &Path, to: &Path, len: usize) -> io::Result<()> {
    let mut input = File::open(from)?.take(len as u64);
    io::copy(&mut input, &mut File::create(to)?)?;
    Ok(())
}

fn concat(parts: &[&str], to: &Path) -> io::Result<()> {
    let mut output = File::create(to)?;
    for part in parts {
        io::copy(&mut File::open(part)?, &mut output)?;
    }
    Ok(())
}

/// `tar cf - src tools docs xtask | xz -T8 -5 > path`
fn tar_xz(root: &Path, path: &Path) -> io::Result<()> {
    let mut tar = Command::new("tar")
        .arg("cf")
        .arg("-")
        .arg("-C")
        .arg(root)
        .args(["src", "tools", "docs", "xtask"])
        .stdout(Stdio::piped())
        .spawn()?;
    let xz = Command::new("xz")
        .args(["-T8", "-5"])
        .stdin(tar.stdout.take().expect("piped"))
        .stdout(File::create(path)?)
        .status()?;
    let tar = tar.wait()?;
    if tar.success() && xz.success() {
        Ok(())
    } else {
        Err(io::Error::other("tar | xz failed"))
    }
}

#[cfg(test)]
mod tests {
    use super::{MIB, write_payload};

    #[test]
    fn the_payload_is_the_same_bytes_every_time() {
        let dir = std::env::temp_dir();
        let (a, b) = (
            dir.join("xtask-payload-a.bin"),
            dir.join("xtask-payload-b.bin"),
        );
        write_payload(&a, 2).unwrap();
        write_payload(&b, 2).unwrap();
        let (a_bytes, b_bytes) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let _ = (std::fs::remove_file(&a), std::fs::remove_file(&b));
        assert_eq!(a_bytes.len(), 2 * MIB);
        assert!(a_bytes == b_bytes);
        // Words and random runs both appear.
        assert!(a_bytes.contains(&b' '));
    }
}
