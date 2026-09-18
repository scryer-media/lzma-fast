//! Builds the LZMA SDK's decoder from the checkout named by `LZMA_SDK`.
//!
//! Every file is checked against the SHA-256 recorded below before it is
//! compiled, so the oracle is the SDK at one commit (0766b733, 26.03) and
//! nothing else. It is built twice, under different symbol prefixes, so both
//! can be linked into one test binary:
//!
//! * `oracle_c_`: `LzmaDec.c` and `Lzma2Dec.c` as plain C, the SDK's own
//!   portable loop.
//! * `oracle_asm_`: the same two files with `Z7_LZMA_DEC_OPT`, which turns the
//!   C loop into an external reference to `LzmaDec_DecodeReal_3`, and the
//!   SDK's assembly for this target to satisfy it. Built where the SDK's
//!   assembly assembles: arm64 Linux with the C compiler, x86-64 Linux with a
//!   JWasm-family assembler, x86-64 Windows with ml64.
//!
//! Without `LZMA_SDK` the crate builds with no oracle and its tests say so
//! and pass; CI sets `LZMA_ORACLE_REQUIRE=1`, and `LZMA_ORACLE_REQUIRE_ASM=1`
//! where the assembly one must be there too, so that a missing oracle fails
//! the build instead of skipping.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use sha2::{Digest, Sha256};

/// The SDK files the oracle compiles or includes, and their SHA-256.
const C_FILES: &[(&str, &str)] = &[
    (
        "C/LzmaDec.c",
        "4e6ec665a6df01f1722e2b2fc36dc5e3aa7c3890e0f8cb11c6a07c9a01f3309b",
    ),
    (
        "C/LzmaDec.h",
        "3aaf07b4ae4173a2d103179455dc7089b5ddbc7fc3db3c0e40964a7499c69266",
    ),
    (
        "C/Lzma2Dec.c",
        "3d691d30e4a1f4b661d56fc483db702167ff6a91781d143a017c793c1860476f",
    ),
    (
        "C/Lzma2Dec.h",
        "a4b97083c3817d3e1e3049f8b1abc0b4ca3e91192606497c02bb5351b57422c7",
    ),
    (
        "C/7zTypes.h",
        "a5d03b8cb65cd3a2a56bde46dd68fb356d2ebacd9d70283e7d4d20b4e0a64713",
    ),
    (
        "C/Precomp.h",
        "c8903f2e36a771a272d8981e1e9ffed5ba16ca3d2f3dca6d1414d2340a6829d0",
    ),
    (
        "C/Compiler.h",
        "d5e42be77d26beaa8c81d3e62ef99fc5685736cfc079779fdb9154d05fa8bc4a",
    ),
    (
        "C/7zWindows.h",
        "996e5dc62a022e05c95142c53729abfdc308b8b006dddbdf78793ff4fb2b8dc6",
    ),
];
const ARM64_FILES: &[(&str, &str)] = &[
    (
        "Asm/arm64/LzmaDecOpt.S",
        "25ee0f34dd5f304ebfce3bb1b016fdee60c876a42e6a90981aa228d59c317552",
    ),
    (
        "Asm/arm64/7zAsm.S",
        "0666cf2e2da64d79bf0b97e70c3d72909a4fe80695c5e491a94994ca4e3ca64c",
    ),
];
const X86_FILES: &[(&str, &str)] = &[
    (
        "Asm/x86/LzmaDecOpt.asm",
        "bddfb31a59c49c8f25f75d19e7330437d2ca3ba81d9655fa427d7585521a3859",
    ),
    (
        "Asm/x86/7zAsm.asm",
        "8a06bb3e5d26ed5b0a311141203469c31ca1119326d6bb12fcc6dc495b94e184",
    ),
];

/// Every function `LzmaDec.c` and `Lzma2Dec.c` export. Each variant renames
/// them all, so the two builds share no symbol. `LzmaDec_DecodeReal_3` is not
/// here: the C build keeps it `static`, and the assembly build must reference
/// it by the name the SDK's assembly defines.
const EXPORTS: &[&str] = &[
    "LzmaDec_InitDicAndState",
    "LzmaDec_Init",
    "LzmaDec_DecodeToDic",
    "LzmaDec_DecodeToBuf",
    "LzmaDec_FreeProbs",
    "LzmaDec_Free",
    "LzmaProps_Decode",
    "LzmaDec_AllocateProbs",
    "LzmaDec_Allocate",
    "LzmaDecode",
    "Lzma2Dec_AllocateProbs",
    "Lzma2Dec_Allocate",
    "Lzma2Dec_Init",
    "Lzma2Dec_DecodeToDic",
    "Lzma2Dec_DecodeToBuf",
    "Lzma2Dec_Parse",
    "Lzma2Decode",
];

fn main() {
    println!("cargo::rustc-check-cfg=cfg(oracle_c)");
    println!("cargo::rustc-check-cfg=cfg(oracle_asm)");
    for var in [
        "LZMA_SDK",
        "LZMA_ORACLE_REQUIRE",
        "LZMA_ORACLE_REQUIRE_ASM",
        "LZMA_ORACLE_ASSEMBLER",
    ] {
        println!("cargo::rerun-if-env-changed={var}");
    }
    println!("cargo::rerun-if-changed=build.rs");

    let require_asm = flag("LZMA_ORACLE_REQUIRE_ASM");
    let require = require_asm || flag("LZMA_ORACLE_REQUIRE");
    let Some(sdk) = env::var_os("LZMA_SDK").map(PathBuf::from) else {
        if require {
            panic!("LZMA_ORACLE_REQUIRE is set but LZMA_SDK is not");
        }
        println!(
            "cargo::warning=LZMA_SDK is not set; the SDK oracle is not built and its tests skip"
        );
        return;
    };

    verify(&sdk, C_FILES);
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    let loop_object = match (arch.as_str(), os.as_str(), target_env.as_str()) {
        ("aarch64", "linux", _) => {
            verify(&sdk, ARM64_FILES);
            Some(assemble_arm64(&sdk, &out))
        }
        ("x86_64", "linux", _) => {
            verify(&sdk, X86_FILES);
            assemble_x86_jwasm(&sdk, &out)
        }
        ("x86_64", "windows", "msvc") => {
            verify(&sdk, X86_FILES);
            Some(assemble_x86_ml64(&sdk, &out))
        }
        _ => None,
    };
    if loop_object.is_none() && require_asm {
        panic!(
            "LZMA_ORACLE_REQUIRE_ASM is set but the SDK's assembly loop cannot be built for {arch}-{os}-{target_env}"
        );
    }

    compile_variant(&sdk, &out, "c", None);
    println!("cargo::rustc-cfg=oracle_c");
    if let Some(object) = loop_object {
        compile_variant(&sdk, &out, "asm", Some(&object));
        println!("cargo::rustc-cfg=oracle_asm");
    }
}

fn flag(name: &str) -> bool {
    env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

fn verify(sdk: &Path, files: &[(&str, &str)]) {
    for (path, expected) in files {
        let full = sdk.join(path);
        println!("cargo::rerun-if-changed={}", full.display());
        let bytes = fs::read(&full).unwrap_or_else(|e| panic!("read {}: {e}", full.display()));
        let actual = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert_eq!(
            actual,
            *expected,
            "{path} in {} is not the pinned SDK's",
            sdk.display()
        );
    }
}

/// Two translation units per variant, one for each SDK file, each renaming the
/// exports and adding the few functions the Rust side calls.
fn compile_variant(sdk: &Path, out: &Path, name: &str, loop_object: Option<&Path>) {
    let prefix = format!("oracle_{name}_");
    let renames = EXPORTS
        .iter()
        .map(|export| format!("#define {export} {prefix}{export}\n"))
        .collect::<String>();
    let alloc = "#include <stdlib.h>\n\
        static void *OracleAlloc(ISzAllocPtr p, size_t size) { (void)p; return malloc(size); }\n\
        static void OracleFree(ISzAllocPtr p, void *address) { (void)p; free(address); }\n\
        static const ISzAlloc g_OracleAlloc = { OracleAlloc, OracleFree };\n";

    let lzma = format!(
        "{renames}#include \"LzmaDec.c\"\n{alloc}\
        void *{prefix}lzma_new(const Byte *props, int *res)\n\
        {{\n\
          CLzmaDec *p = (CLzmaDec *)malloc(sizeof(CLzmaDec));\n\
          if (!p) {{ *res = SZ_ERROR_MEM; return NULL; }}\n\
          LzmaDec_Construct(p);\n\
          *res = LzmaDec_Allocate(p, props, LZMA_PROPS_SIZE, &g_OracleAlloc);\n\
          if (*res != SZ_OK) {{ free(p); return NULL; }}\n\
          LzmaDec_Init(p);\n\
          return p;\n\
        }}\n\
        int {prefix}lzma_decode(void *p, Byte *dest, size_t *destLen, const Byte *src, size_t *srcLen, int finishEnd, int *status)\n\
        {{\n\
          ELzmaStatus s = LZMA_STATUS_NOT_SPECIFIED;\n\
          SRes res = LzmaDec_DecodeToBuf((CLzmaDec *)p, dest, destLen, src, srcLen, finishEnd ? LZMA_FINISH_END : LZMA_FINISH_ANY, &s);\n\
          *status = (int)s;\n\
          return res;\n\
        }}\n\
        void {prefix}lzma_free(void *p)\n\
        {{\n\
          LzmaDec_Free((CLzmaDec *)p, &g_OracleAlloc);\n\
          free(p);\n\
        }}\n"
    );
    let lzma2 = format!(
        "{renames}#include \"Lzma2Dec.c\"\n{alloc}\
        void *{prefix}lzma2_new(Byte prop, int *res)\n\
        {{\n\
          CLzma2Dec *p = (CLzma2Dec *)malloc(sizeof(CLzma2Dec));\n\
          if (!p) {{ *res = SZ_ERROR_MEM; return NULL; }}\n\
          Lzma2Dec_Construct(p);\n\
          *res = Lzma2Dec_Allocate(p, prop, &g_OracleAlloc);\n\
          if (*res != SZ_OK) {{ free(p); return NULL; }}\n\
          Lzma2Dec_Init(p);\n\
          return p;\n\
        }}\n\
        int {prefix}lzma2_decode(void *p, Byte *dest, size_t *destLen, const Byte *src, size_t *srcLen, int finishEnd, int *status)\n\
        {{\n\
          ELzmaStatus s = LZMA_STATUS_NOT_SPECIFIED;\n\
          SRes res = Lzma2Dec_DecodeToBuf((CLzma2Dec *)p, dest, destLen, src, srcLen, finishEnd ? LZMA_FINISH_END : LZMA_FINISH_ANY, &s);\n\
          *status = (int)s;\n\
          return res;\n\
        }}\n\
        void {prefix}lzma2_free(void *p)\n\
        {{\n\
          Lzma2Dec_Free((CLzma2Dec *)p, &g_OracleAlloc);\n\
          free(p);\n\
        }}\n"
    );

    let lzma_path = out.join(format!("{prefix}lzma.c"));
    let lzma2_path = out.join(format!("{prefix}lzma2.c"));
    fs::write(&lzma_path, lzma).expect("write wrapper");
    fs::write(&lzma2_path, lzma2).expect("write wrapper");

    let mut build = cc::Build::new();
    build
        .include(sdk.join("C"))
        .file(&lzma_path)
        .file(&lzma2_path)
        .opt_level(2)
        .warnings(false);
    if let Some(object) = loop_object {
        build.define("Z7_LZMA_DEC_OPT", None).object(object);
    }
    build.compile(&format!("{prefix}sdk"));
}

fn assemble_arm64(sdk: &Path, out: &Path) -> PathBuf {
    let objects = cc::Build::new()
        .include(sdk.join("Asm/arm64"))
        .file(sdk.join("Asm/arm64/LzmaDecOpt.S"))
        .warnings(false)
        .compile_intermediates();
    let [object] = objects.as_slice() else {
        panic!("expected one object from LzmaDecOpt.S, got {objects:?}");
    };
    let kept = out.join("sdk_LzmaDecOpt_arm64.o");
    fs::copy(object, &kept).expect("copy loop object");
    kept
}

/// The SDK's own Linux build assembles `LzmaDecOpt.asm` with a JWasm-family
/// assembler (`7zip_gcc.mak`): `-elf64 -DABI_LINUX` selects System V.
fn assemble_x86_jwasm(sdk: &Path, out: &Path) -> Option<PathBuf> {
    let assembler = env::var_os("LZMA_ORACLE_ASSEMBLER")
        .map(PathBuf::from)
        .or_else(|| {
            ["jwasm", "uasm", "asmc"]
                .iter()
                .map(PathBuf::from)
                .find(|name| Command::new(name).arg("-?").output().is_ok())
        })?;
    let dir = sdk.join("Asm/x86");
    let object = out.join("sdk_LzmaDecOpt_sysv.o");
    run(Command::new(&assembler)
        .args(["-nologo", "-elf64", "-DABI_LINUX"])
        .arg(format!("-I{}", dir.display()))
        .arg(format!("-Fo{}", object.display()))
        .arg(dir.join("LzmaDecOpt.asm")));
    Some(object)
}

/// As the SDK's MSVC makefiles do.
fn assemble_x86_ml64(sdk: &Path, out: &Path) -> PathBuf {
    let target = env::var("TARGET").expect("TARGET");
    let mut command =
        cc::windows_registry::find(&target, "ml64.exe").expect("ml64.exe from the MSVC toolchain");
    let dir = sdk.join("Asm/x86");
    let object = out.join("sdk_LzmaDecOpt_win64.obj");
    run(command
        .args(["/nologo", "/c"])
        .arg(format!("/I{}", dir.display()))
        .arg(format!("/Fo{}", object.display()))
        .arg(dir.join("LzmaDecOpt.asm")));
    object
}

fn run(command: &mut Command) {
    let output = command
        .output()
        .unwrap_or_else(|e| panic!("run {command:?}: {e}"));
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
