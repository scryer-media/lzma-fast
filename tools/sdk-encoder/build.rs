//! Builds the LZMA SDK's *encoder* from the checkout named by `LZMA_SDK`.
//!
//! The decoder has `tools/sdk-oracle`, which links `LzmaDec.c` and
//! `Lzma2Dec.c` into the test binary so a fuzz target can drive the C and the
//! Rust call for call. This is the same thing for the other direction:
//! `LzmaEnc.c`, `Lzma2Enc.c` and the match finders behind them, plus the two
//! filters that sit in front of them, linked as a library.
//!
//! It is not the same code path as `cargo xtask lzma-util`, which builds the
//! SDK's encoder as *binaries* driven over the command line. Those suit the
//! parity tests, which run a fixed list of settings over a fixed corpus; a
//! fuzzer that spawns a process and writes two temporary files per case would
//! spend all its time in the kernel. This one is in-process.
//!
//! Every file is checked against the SHA-256 recorded below before it is
//! compiled, so the reference is the SDK at one commit (0766b733, 26.03) and
//! nothing else, exactly as `tools/sdk-oracle` does it.
//!
//! There is no `Z7_ST` here: the point is to compare the threaded match
//! finder and the block-parallel coder too, and `Z7_ST` is what compiles them
//! out.
//!
//! Without `LZMA_SDK` the crate builds with no encoder and its callers say so
//! and pass; CI sets `LZMA_ENCODER_ORACLE_REQUIRE=1`, so that a missing SDK
//! fails the build instead of quietly fuzzing the crate against itself.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

/// The SDK sources this compiles, and their SHA-256.
const SOURCES: &[(&str, &str)] = &[
    (
        "C/Alloc.c",
        "b23e008fdaca355bc566e696223655ecf5758e0233d644449946bb7c93127f71",
    ),
    (
        "C/CpuArch.c",
        "1e0f166e34946fc7da05647a85d8edeb64038282915e73eef676cd067d6a7527",
    ),
    (
        "C/LzFind.c",
        "f73da68845094a85b006a0d2da80b3ecdec92a1ca0f85df637e565c642e8d5ba",
    ),
    (
        "C/LzFindMt.c",
        "6b04cd48e97c14a26f01865adde16440160f367e72a7b1ffda8ca084472ae223",
    ),
    (
        "C/LzFindOpt.c",
        "c8ac04141e38d825e74984e924842e76fc903d74472dd8354470f23ecbfd853a",
    ),
    (
        "C/LzmaEnc.c",
        "67f23656339b5e19015acac41acbcf36ea7787a8419907dce91c1dd336a2fb0f",
    ),
    (
        "C/Lzma2Enc.c",
        "4f53b9c04a7eca3c688b77ad869df26e5ed1d41f7e026295c642398457fbc6cc",
    ),
    (
        "C/MtCoder.c",
        "bbb952acf0bb36ff2c7d5a79204696a3a7d8ac53ba3588d093541a5e016798d0",
    ),
    (
        "C/MtDec.c",
        "0ebeaf78be22e016702865507afbd1c94e6e76dbfd4e1d8a123e4c41e9bdcbcf",
    ),
    (
        "C/Threads.c",
        "db0a1f73472032a3e8a41372dd59606f07133c366cc0dd9c4e92e1e12f51e747",
    ),
    (
        "C/7zStream.c",
        "835941df3324d828770bc09749a976bad8449e3733896d51bf102557c9ac0acb",
    ),
    (
        "C/Bra.c",
        "c90cbc0747a406d056daf3d7b2bf05cf88c55529777469db5ca15f754b7c38e8",
    ),
    (
        "C/Bra86.c",
        "0925d655955bdd2b7b25a01bbb3d2d395e5bac6d572f2cfd273141d1896af0ec",
    ),
    (
        "C/BraIA64.c",
        "29e249368a098988bee5585a7f0225b9067155e46b8e16d606c5e63e274e9c30",
    ),
    (
        "C/Delta.c",
        "d5b3fad0896c21b428c9803760e537a30b82ed344faa525d39ca9a4b555a80f2",
    ),
];

/// The headers those sources include from the SDK. They are not compiled, but
/// the reference is only pinned if they are pinned too.
const HEADERS: &[(&str, &str)] = &[
    (
        "C/Alloc.h",
        "f7c1f47094526f8a4d8a6a9bbfac2ae5ffb2f4e39124129890eb962db4c14d71",
    ),
    (
        "C/CpuArch.h",
        "837c38beee7db282c9dfbd5a47ac1519a825788860c9cc68623376698caf3e2b",
    ),
    (
        "C/LzFind.h",
        "42732b38df9bb18f82866d7deaa8663f6c6f9cb10e2a2cdfa9e6f420c2e2f239",
    ),
    (
        "C/LzFindMt.h",
        "1851370be11bd1b998974a094f1997a34da91ad6474641c27bbd83a9a38fb4a8",
    ),
    (
        "C/LzHash.h",
        "42d146d2130dfe49d0b21d15cc6f0fe9d79ac0ebb629985fe3aedde7b2bee80e",
    ),
    (
        "C/LzmaEnc.h",
        "c5e78398309363dd181840b7ba0bcb856f66619f755d5e2f5e04f7437186ba5f",
    ),
    (
        "C/Lzma2Enc.h",
        "d0fc59677e8e2b51e7182916a65a09bdd1625b29a9621c0042ae8fc64cf1e919",
    ),
    (
        "C/MtCoder.h",
        "e14b8bccded0f359f89be0ed48bdc3fcdd58f2cfe39eba3adf6bccf4dd65405d",
    ),
    (
        "C/MtDec.h",
        "c5eb7b409ef86b15f03b8c7c1d58668e89d864f0e19baa2a3fe9fd41819b1df0",
    ),
    (
        "C/Threads.h",
        "166b4d1ae24f650513e0cc8421af1656ad36aafc9e9a6c9984ec36d33e787b4f",
    ),
    (
        "C/Bra.h",
        "bf4addce6209562a700edf040a8efc5b392bb33d4427b1513b6e3a877d04f431",
    ),
    (
        "C/Delta.h",
        "ce359ff7eb4c9d806777e21d0ec0416cf06c1f556242cbbcf565492b47141eeb",
    ),
    (
        "C/7zTypes.h",
        "a5d03b8cb65cd3a2a56bde46dd68fb356d2ebacd9d70283e7d4d20b4e0a64713",
    ),
    (
        "C/7zWindows.h",
        "996e5dc62a022e05c95142c53729abfdc308b8b006dddbdf78793ff4fb2b8dc6",
    ),
    (
        "C/Precomp.h",
        "c8903f2e36a771a272d8981e1e9ffed5ba16ca3d2f3dca6d1414d2340a6829d0",
    ),
    (
        "C/Compiler.h",
        "d5e42be77d26beaa8c81d3e62ef99fc5685736cfc079779fdb9154d05fa8bc4a",
    ),
];

/// The wrapper the Rust side calls. Everything it takes is a plain value or a
/// borrowed buffer; everything it returns is one `malloc`ed block freed by
/// `sdk_enc_free`, so the Rust side never has to know how the SDK allocates.
const WRAPPER_C: &str = r#"/* An in-process wrapper over the pinned SDK's encoder, for differential
   testing. The settings mirror `cargo xtask lzma-util`'s oracles exactly, so
   that a fuzz finding and a parity-test failure mean the same thing. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "LzmaEnc.h"
#include "Lzma2Enc.h"
#include "Bra.h"
#include "Delta.h"
#include "Alloc.h"

typedef struct {
  int level, btMode, numHashBytes, lc, lp, pb;
  unsigned fb;
  unsigned dictSize;
  int numThreads;
} SdkEncProps;

/* A growable sink. `Write` returning less than `size` is how the SDK learns a
   sink failed, which is what an allocation failure has to look like here. */
typedef struct { ISeqOutStream vt; Byte *buf; size_t len, cap; } MemOut;
static size_t MemOut_Write(ISeqOutStreamPtr pp, const void *buf, size_t size) {
  MemOut *s = Z7_CONTAINER_FROM_VTBL(pp, MemOut, vt);
  if (s->len + size > s->cap) {
    size_t cap = s->cap ? s->cap : 1024;
    while (cap < s->len + size) cap *= 2;
    Byte *grown = (Byte *)realloc(s->buf, cap);
    if (!grown) return 0;
    s->buf = grown; s->cap = cap;
  }
  memcpy(s->buf + s->len, buf, size);
  s->len += size;
  return size;
}
static void MemOut_Init(MemOut *s) {
  s->vt.Write = MemOut_Write; s->buf = NULL; s->len = 0; s->cap = 0;
}

/* LZMA1 reads its input through a stream, as `lzma-oracle` does. */
typedef struct { ISeqInStream vt; const Byte *p; size_t rem; } MemIn;
static SRes MemIn_Read(ISeqInStreamPtr pp, void *buf, size_t *size) {
  MemIn *s = Z7_CONTAINER_FROM_VTBL(pp, MemIn, vt);
  size_t n = *size; if (n > s->rem) n = s->rem;
  memcpy(buf, s->p, n); s->p += n; s->rem -= n; *size = n; return SZ_OK;
}

static void apply(const SdkEncProps *p, CLzmaEncProps *props) {
  LzmaEncProps_Init(props);
  props->level = p->level;
  props->btMode = p->btMode;
  props->numHashBytes = p->numHashBytes;
  props->lc = p->lc; props->lp = p->lp; props->pb = p->pb;
  props->fb = (int)p->fb;
  props->dictSize = (UInt32)p->dictSize;
  props->numThreads = p->numThreads;
}

/* LZMA-Alone: the 5 property bytes, the 8-byte size, then the stream. This is
   `lzma-oracle.c` and `lzma-oracle-mt.c` byte for byte. */
int sdk_enc_lzma1(const Byte *src, size_t srcLen, const SdkEncProps *p,
                  Byte **out, size_t *outLen) {
  CLzmaEncProps props; apply(p, &props);
  CLzmaEncHandle enc = LzmaEnc_Create(&g_Alloc);
  if (!enc) return SZ_ERROR_MEM;
  {
    SRes res = LzmaEnc_SetProps(enc, &props);
    if (res != SZ_OK) { LzmaEnc_Destroy(enc, &g_Alloc, &g_Alloc); return res; }
  }
  {
    MemOut sink; MemOut_Init(&sink);
    Byte header[LZMA_PROPS_SIZE + 8]; size_t hs = LZMA_PROPS_SIZE;
    SRes res = LzmaEnc_WriteProperties(enc, header, &hs);
    if (res == SZ_OK) {
      int i;
      for (i = 0; i < 8; i++) header[hs++] = (Byte)((UInt64)srcLen >> (8 * i));
      if (MemOut_Write(&sink.vt, header, hs) != hs) res = SZ_ERROR_MEM;
    }
    if (res == SZ_OK) {
      MemIn in; in.vt.Read = MemIn_Read; in.p = src; in.rem = srcLen;
      res = LzmaEnc_Encode(enc, &sink.vt, &in.vt, NULL, &g_Alloc, &g_Alloc);
    }
    LzmaEnc_Destroy(enc, &g_Alloc, &g_Alloc);
    if (res != SZ_OK) { free(sink.buf); return res; }
    *out = sink.buf; *outLen = sink.len;
    return SZ_OK;
  }
}

/* Raw LZMA2: the property byte through `prop`, the stream through `out`. A
   `blockSize` of 0 means solid, as in `lzma2-oracle-mt.c`. */
int sdk_enc_lzma2(const Byte *src, size_t srcLen, const SdkEncProps *p,
                  UInt64 blockSize, int blockThreads,
                  Byte *prop, Byte **out, size_t *outLen) {
  CLzma2EncProps props; Lzma2EncProps_Init(&props);
  apply(p, &props.lzmaProps);
  props.blockSize = blockSize ? blockSize : LZMA2_ENC_PROPS_BLOCK_SIZE_SOLID;
  props.numBlockThreads_Max = blockThreads;
  props.numBlockThreads_Reduced = blockThreads;
  props.numTotalThreads = blockThreads * p->numThreads;
  {
    CLzma2EncHandle enc = Lzma2Enc_Create(&g_Alloc, &g_Alloc);
    if (!enc) return SZ_ERROR_MEM;
    {
      SRes res = Lzma2Enc_SetProps(enc, &props);
      if (res != SZ_OK) { Lzma2Enc_Destroy(enc); return res; }
    }
    Lzma2Enc_SetDataSize(enc, (UInt64)srcLen);
    *prop = Lzma2Enc_WriteProperties(enc);
    {
      MemOut sink; MemOut_Init(&sink);
      SRes res = Lzma2Enc_Encode2(enc, &sink.vt, NULL, NULL, NULL, src, srcLen, NULL);
      Lzma2Enc_Destroy(enc);
      if (res != SZ_OK) { free(sink.buf); return res; }
      *out = sink.buf; *outLen = sink.len;
      return SZ_OK;
    }
  }
}

void sdk_enc_free(Byte *p) { free(p); }

/* The two filters that sit in front of the coder in an .xz chain. */
void sdk_filter_delta_enc(Byte *data, size_t size, unsigned delta) {
  Byte state[DELTA_STATE_SIZE];
  Delta_Init(state);
  Delta_Encode(state, delta, data, size);
}
void sdk_filter_x86_enc(Byte *data, size_t size, UInt32 pc) {
  UInt32 state = Z7_BRANCH_CONV_ST_X86_STATE_INIT_VAL;
  z7_BranchConvSt_X86_Enc(data, size, pc, &state);
}
"#;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(sdk_encoder)");
    for var in ["LZMA_SDK", "LZMA_ENCODER_ORACLE_REQUIRE"] {
        println!("cargo::rerun-if-env-changed={var}");
    }
    println!("cargo::rerun-if-changed=build.rs");

    let require = env::var("LZMA_ENCODER_ORACLE_REQUIRE").is_ok_and(|v| !v.is_empty() && v != "0");
    let Some(sdk) = env::var_os("LZMA_SDK").map(PathBuf::from) else {
        assert!(
            !require,
            "LZMA_ENCODER_ORACLE_REQUIRE is set but LZMA_SDK is not"
        );
        println!(
            "cargo::warning=LZMA_SDK is not set; the SDK encoder is not built and its callers skip"
        );
        return;
    };

    verify(&sdk, SOURCES);
    verify(&sdk, HEADERS);

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let wrapper = out.join("sdk_encoder.c");
    fs::write(&wrapper, WRAPPER_C).expect("write the wrapper");

    let mut build = cc::Build::new();
    build
        .include(sdk.join("C"))
        .file(&wrapper)
        .opt_level(2)
        .warnings(false);
    for (path, _) in SOURCES {
        build.file(sdk.join(path));
    }
    // No `Z7_ST`: `LzFindMt.c` and `MtCoder.c` are the point. They need a
    // thread library, which on Windows is the one every program links.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        build.flag("-pthread");
    }
    build.compile("sdk_encoder");

    println!("cargo::rustc-cfg=sdk_encoder");
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
