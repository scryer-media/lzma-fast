//! Proves that the decode loops this crate ships are the LZMA SDK's.
//!
//! The three files under `src/lzma/decode_opt/` are hand translations of
//! `Asm/arm64/LzmaDecOpt.S` and `Asm/x86/LzmaDecOpt.asm`. Their headers list
//! the edits the translation needed and say nothing else changed. This tool
//! turns that claim into a check. It assembles the SDK's own files with the
//! SDK's own assemblers, compiles this crate for every target that carries a
//! loop, pulls the function out of both objects, and compares them:
//!
//! * arm64 must be byte-identical. Instructions are fixed-width, so two
//!   assemblers given the same source produce the same bytes, and anything
//!   else means the source differs.
//! * x86-64 must decode to the same instruction stream. MASM-family
//!   assemblers and LLVM may pick different encodings for one instruction
//!   (`8B C1` and `89 C8` are both `mov eax,ecx`), different branch widths and
//!   different padding, so the stream is compared after dropping NOPs and
//!   rewriting every branch target as the index of the instruction it lands
//!   on. Byte identity is reported when it happens, but not required.
//!
//! Either way the extracted function must carry no relocations: the loop is
//! self-contained, so a relocation would mean its bytes are not final.
//!
//! The SDK files are pinned by SHA-256 below, so the reference itself cannot
//! drift. A bump to a new SDK release is a deliberate edit to these hashes.

mod extract;
mod x86;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

use sha2::{Digest, Sha256};

use crate::extract::Function;

/// The SDK release the translations were made from.
const SDK_RELEASE: &str = "26.03 (github.com/ip7z/7zip commit 0766b733)";

/// SHA-256 of every SDK file the reference objects are built from.
const PINNED: &[(&str, &str)] = &[
    (
        "Asm/arm64/LzmaDecOpt.S",
        "25ee0f34dd5f304ebfce3bb1b016fdee60c876a42e6a90981aa228d59c317552",
    ),
    (
        "Asm/arm64/7zAsm.S",
        "0666cf2e2da64d79bf0b97e70c3d72909a4fe80695c5e491a94994ca4e3ca64c",
    ),
    (
        "Asm/x86/LzmaDecOpt.asm",
        "bddfb31a59c49c8f25f75d19e7330437d2ca3ba81d9655fa427d7585521a3859",
    ),
    (
        "Asm/x86/7zAsm.asm",
        "8a06bb3e5d26ed5b0a311141203469c31ca1119326d6bb12fcc6dc495b94e184",
    ),
];

const USAGE: &str = "\
cargo run -p asm-provenance -- --sdk <7zip checkout> [options]

  --sdk <dir>            checkout of github.com/ip7z/7zip at the pinned release
  --target <triple>      a target to check; repeatable. Default: every target
                         that carries a loop (the Rust standard library for
                         each must be installed; nothing is linked)
  --arm64-cc <program>   assemble the arm64 reference with this C compiler
                         driver; repeatable. Default: clang
  --x86 <kind>=<program> assemble the x86-64 reference with this assembler,
                         where kind is ml64, jwasm, uasm or asmc; repeatable
  --x86-object <abi>=<file>
                         an x86-64 reference object built elsewhere, where abi
                         is win64 or sysv; repeatable
  --work <dir>           scratch directory. Default: target/asm-provenance

Every x86-64 target needs at least one --x86 or --x86-object for its ABI.";

const DEFAULT_TARGETS: &[&str] = &[
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "aarch64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Arch {
    Aarch64,
    X86_64,
}

/// The x86-64 calling convention a loop was written for. The SDK builds one
/// source both ways, with `ABI_LINUX` selecting System V. arm64 has one file
/// for every platform and is recorded as `SysV` throughout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Abi {
    Win64,
    SysV,
}

impl Abi {
    fn name(self) -> &'static str {
        match self {
            Abi::Win64 => "win64",
            Abi::SysV => "sysv",
        }
    }
}

struct Target {
    triple: String,
    arch: Arch,
    abi: Abi,
}

impl Target {
    fn parse(triple: &str) -> Option<Self> {
        let arch = if triple.starts_with("aarch64-") {
            Arch::Aarch64
        } else if triple.starts_with("x86_64-") {
            Arch::X86_64
        } else {
            return None;
        };
        // `src/lzma/decode_opt/x86_64.rs` picks the file by `target_os`.
        let abi = if arch == Arch::X86_64 && triple.contains("-windows") {
            Abi::Win64
        } else {
            Abi::SysV
        };
        Some(Self {
            triple: triple.to_owned(),
            arch,
            abi,
        })
    }
}

/// One way of producing a reference object.
struct Reference {
    /// What produced it, for the report.
    label: String,
    arch: Arch,
    abi: Abi,
    function: Function,
}

struct Options {
    sdk: PathBuf,
    work: PathBuf,
    targets: Vec<Target>,
    arm64_cc: Vec<String>,
    x86: Vec<(String, String)>,
    x86_objects: Vec<(Abi, PathBuf)>,
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("asm-provenance: {message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("asm-provenance: {message}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Options, String> {
    let mut sdk = None;
    let mut work = None;
    let mut targets = Vec::new();
    let mut arm64_cc = Vec::new();
    let mut x86 = Vec::new();
    let mut x86_objects = Vec::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--sdk" => sdk = Some(PathBuf::from(value()?)),
            "--work" => work = Some(PathBuf::from(value()?)),
            "--target" => {
                let triple = value()?;
                targets.push(
                    Target::parse(&triple)
                        .ok_or_else(|| format!("{triple} carries no assembly loop"))?,
                );
            }
            "--arm64-cc" => arm64_cc.push(value()?),
            "--x86" => {
                let spec = value()?;
                let (kind, program) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--x86 wants kind=program, got {spec}"))?;
                if !matches!(kind, "ml64" | "jwasm" | "uasm" | "asmc") {
                    return Err(format!("unknown x86 assembler kind {kind}"));
                }
                x86.push((kind.to_owned(), program.to_owned()));
            }
            "--x86-object" => {
                let spec = value()?;
                let (abi, file) = spec
                    .split_once('=')
                    .ok_or_else(|| format!("--x86-object wants abi=file, got {spec}"))?;
                let abi = match abi {
                    "win64" => Abi::Win64,
                    "sysv" => Abi::SysV,
                    other => return Err(format!("unknown ABI {other}")),
                };
                x86_objects.push((abi, PathBuf::from(file)));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if targets.is_empty() {
        targets = DEFAULT_TARGETS
            .iter()
            .map(|triple| Target::parse(triple).expect("a default target"))
            .collect();
    }
    if arm64_cc.is_empty() {
        arm64_cc.push("clang".to_owned());
    }
    Ok(Options {
        sdk: sdk.ok_or("--sdk is required")?,
        work: work.unwrap_or_else(|| repo_root().join("target").join("asm-provenance")),
        targets,
        arm64_cc,
        x86,
        x86_objects,
    })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("tools/asm-provenance sits two levels under the repository root")
        .to_path_buf()
}

fn run(options: &Options) -> Result<bool, String> {
    fs::create_dir_all(&options.work)
        .map_err(|e| format!("create {}: {e}", options.work.display()))?;

    println!("reference: LZMA SDK {SDK_RELEASE}");
    verify_pins(&options.sdk)?;

    let wants_arm64 = options.targets.iter().any(|t| t.arch == Arch::Aarch64);
    let wants_x86 = options.targets.iter().any(|t| t.arch == Arch::X86_64);

    let mut references = Vec::new();
    if wants_arm64 {
        for cc in &options.arm64_cc {
            references.push(assemble_arm64(options, cc)?);
        }
    }
    if wants_x86 {
        for (kind, program) in &options.x86 {
            for abi in [Abi::Win64, Abi::SysV] {
                references.push(assemble_x86(options, kind, program, abi)?);
            }
        }
        for (abi, file) in &options.x86_objects {
            references.push(Reference {
                label: format!("SDK x86 {} prebuilt {}", abi.name(), file.display()),
                arch: Arch::X86_64,
                abi: *abi,
                function: extract::reference(file)?,
            });
        }
    }

    let mut ok = true;
    // One source file must produce one function whatever the object format,
    // so each build is also checked against the first build of its file.
    let mut seen: Vec<(Arch, Abi, String, Vec<u8>)> = Vec::new();
    for target in &options.targets {
        let ours = build_ours(options, target)?;
        println!(
            "\n{}: {} bytes, sha256 {}",
            target.triple,
            ours.bytes.len(),
            hex(&Sha256::digest(&ours.bytes))
        );
        match seen
            .iter()
            .find(|(arch, abi, ..)| *arch == target.arch && *abi == target.abi)
        {
            Some((.., first, bytes)) if *bytes == ours.bytes => {
                println!("  same bytes as the {first} build");
            }
            Some((.., first, _)) => {
                println!("  FAIL: differs from the {first} build of the same source");
                ok = false;
            }
            None => seen.push((
                target.arch,
                target.abi,
                target.triple.clone(),
                ours.bytes.clone(),
            )),
        }

        let candidates: Vec<&Reference> = references
            .iter()
            .filter(|r| r.arch == target.arch && r.abi == target.abi)
            .collect();
        if candidates.is_empty() {
            println!("  FAIL: no reference object for this target; see --x86 / --x86-object");
            ok = false;
        }
        for reference in candidates {
            ok &= compare(target, &ours, reference);
        }
    }

    println!(
        "\n{}",
        if ok {
            "asm-provenance: every loop checked is the SDK's"
        } else {
            "asm-provenance: MISMATCH"
        }
    );
    Ok(ok)
}

fn verify_pins(sdk: &Path) -> Result<(), String> {
    for (name, expected) in PINNED {
        let path = sdk.join(name);
        let bytes = fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let actual = hex(&Sha256::digest(&bytes));
        if actual != *expected {
            return Err(format!(
                "{name} is not the pinned file: sha256 {actual}, expected {expected}"
            ));
        }
        println!("  pinned  {name}  {actual}");
    }
    Ok(())
}

fn compare(target: &Target, ours: &Function, reference: &Reference) -> bool {
    let theirs = &reference.function;
    if ours.bytes == theirs.bytes {
        println!("  PASS byte-identical to {}", reference.label);
        return true;
    }
    match target.arch {
        Arch::Aarch64 => {
            let at = ours
                .bytes
                .iter()
                .zip(&theirs.bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(ours.bytes.len().min(theirs.bytes.len()));
            println!(
                "  FAIL differs from {} ({} vs {} bytes, first difference at 0x{at:x})",
                reference.label,
                ours.bytes.len(),
                theirs.bytes.len()
            );
            false
        }
        Arch::X86_64 => match x86::compare(&ours.bytes, &theirs.bytes) {
            Ok(report) => {
                println!(
                    "  PASS same {} instructions as {} ({} encoded differently; NOPs {} ours, {} theirs; {} vs {} bytes)",
                    report.instructions,
                    reference.label,
                    report.encodings_differ,
                    report.nops_ours,
                    report.nops_theirs,
                    ours.bytes.len(),
                    theirs.bytes.len()
                );
                true
            }
            Err(message) => {
                println!("  FAIL differs from {}:\n{message}", reference.label);
                false
            }
        },
    }
}

/// Compiles the crate with only the `asm` feature for `target` and returns
/// the loop's bytes. Nothing is linked, so the target's standard library is
/// all that is needed, not a toolchain for it.
fn build_ours(options: &Options, target: &Target) -> Result<Function, String> {
    let object = options.work.join(format!("ours-{}.o", target.triple));
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .current_dir(repo_root())
        // Fat LTO would leave bitcode in the object instead of machine code.
        .env("CARGO_PROFILE_RELEASE_LTO", "false")
        .args([
            "rustc",
            "--quiet",
            "--locked",
            "-p",
            "lzma-turbo",
            "--lib",
            "--release",
            "--no-default-features",
            "--features",
            "asm",
            "--target",
            &target.triple,
            "--target-dir",
        ])
        .arg(options.work.join("target"))
        .args(["--", "-C", "codegen-units=1"])
        .arg(format!("--emit=obj={}", object.display()))
        .status()
        .map_err(|e| format!("run cargo: {e}"))?;
    if !status.success() {
        return Err(format!(
            "building lzma-turbo for {0} failed; is its standard library installed (`rustup target add {0}`)?",
            target.triple
        ));
    }
    extract::ours(&object)
}

fn assemble_arm64(options: &Options, cc: &str) -> Result<Reference, String> {
    let dir = options.sdk.join("Asm").join("arm64");
    let name = Path::new(cc)
        .file_name()
        .map_or_else(|| cc.to_owned(), |n| n.to_string_lossy().into_owned());
    let object = options.work.join(format!("sdk-arm64-{name}.o"));
    // The SDK's makefiles assemble the file through the C driver, which runs
    // the preprocessor over its `#define`d register names. An ELF target keeps
    // the SDK's own label spelling valid; GCC can only do that natively.
    let mut command = Command::new(cc);
    if name.contains("clang") {
        command.arg("--target=aarch64-linux-gnu");
    } else if !cfg!(all(target_arch = "aarch64", target_os = "linux")) {
        return Err(format!(
            "{cc} can only assemble the arm64 reference on arm64 Linux"
        ));
    }
    command
        .arg("-c")
        .arg(dir.join("LzmaDecOpt.S"))
        .arg("-I")
        .arg(&dir)
        .arg("-o")
        .arg(&object);
    run_tool(command, cc)?;
    Ok(Reference {
        label: format!("SDK arm64 via {}", version_line(cc, &["--version"])),
        arch: Arch::Aarch64,
        abi: Abi::SysV,
        function: extract::reference(&object)?,
    })
}

fn assemble_x86(
    options: &Options,
    kind: &str,
    program: &str,
    abi: Abi,
) -> Result<Reference, String> {
    let dir = options.sdk.join("Asm").join("x86");
    let object = options
        .work
        .join(format!("sdk-x86-{kind}-{}.o", abi.name()));
    let mut command = Command::new(program);
    // The flags are the SDK's own: `CPP/7zip/7zip_gcc.mak` for the JWasm
    // family (`-elf64 -DABI_LINUX` on Linux), the MSVC makefiles for ml64.
    match kind {
        "ml64" => {
            command.args(["/nologo", "/c"]);
            if abi == Abi::SysV {
                command.arg("/DABI_LINUX");
            }
            command
                .arg(format!("/I{}", dir.display()))
                .arg(format!("/Fo{}", object.display()));
        }
        _ => {
            command.arg("-nologo");
            match abi {
                Abi::Win64 => command.arg("-win64"),
                Abi::SysV => command.args(["-elf64", "-DABI_LINUX"]),
            };
            command
                .arg(format!("-I{}", dir.display()))
                .arg(format!("-Fo{}", object.display()));
        }
    }
    command.arg(dir.join("LzmaDecOpt.asm"));
    run_tool(command, program)?;
    let version = match kind {
        "ml64" => version_line(program, &[]),
        _ => version_line(program, &["-?"]),
    };
    Ok(Reference {
        label: format!("SDK x86 {} via {kind} ({version})", abi.name()),
        arch: Arch::X86_64,
        abi,
        function: extract::reference(&object)?,
    })
}

fn run_tool(mut command: Command, program: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|e| format!("run {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// The first line a tool prints about itself, for the report.
fn version_line(program: &str, args: &[&str]) -> String {
    let Ok(output) = Command::new(program).args(args).output() else {
        return program.to_owned();
    };
    let text = [output.stdout, output.stderr].concat();
    String::from_utf8_lossy(&text)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(program)
        .to_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
