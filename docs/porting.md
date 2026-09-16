# Porting LzmaDec.c to Rust

## Reference

- Source tree: `~/dev/supporting-codebases/7zip` (github.com/ip7z/7zip),
  commit `0766b733fe3e06dd2a7f9a3cfbf2108ac73abd17`, tag 26.03.
- Files, all public domain (`C/` and `Asm/` in the LZMA SDK):

| C file | Lines | Role | Rust target |
| --- | --- | --- | --- |
| `C/LzmaDec.h` | – | state struct, props, API | `src/lzma/state.rs` |
| `C/LzmaDec.c` | 1367 | `LzmaDec_DecodeReal` (fast loop), `LzmaDec_TryDummy` (careful path), `LzmaDec_DecodeToDic`, `LzmaDec_DecodeToBuf` | `src/lzma/decode.rs`, `src/lzma/dummy.rs`, `src/lzma/mod.rs` |
| `C/Lzma2Dec.h` / `C/Lzma2Dec.c` | 493 | LZMA2 chunk framing, prop/dict resets | `src/lzma2/mod.rs` |
| `C/7zTypes.h`, `C/Compiler.h`, `C/Precomp.h` | – | macros (`kNumBitModelTotalBits`, `Z7_...`) | constants in `src/lzma/consts.rs` |
| `C/Util/Lzma/LzmaUtil.c` | – | `.lzma` header parse, streaming loop | `tools/lzma-bench` and tests |

Not in scope for the single-threaded port: `Lzma2DecMt.c`, `MtDec.c`,
`XzDec.c`, `7zDec.c`, and the `Asm/x86/LzmaDecOpt.asm` / `Asm/arm64/LzmaDecOpt.S`
hand-written loops. The C fast loop compiled with the toolchain defaults is
the parity target for the port; the asm loop is what `7zz` ships and is the
acceptance target for the optimization phase.

## Rules

1. Faithful first. Port `LzmaDec_DecodeReal` as one function with the same
   locals (`range`, `code`, `buf`, `probs`, `dic`, `dicPos`, `processedPos`,
   `checkDicSize`, `state`, `rep0..rep3`, `limit`, `len`), the same macro
   expansions (`NORMALIZE`, `IF_BIT_0`, `UPDATE_0`, `UPDATE_1`, `TREE_DECODE`,
   `MATCHED_LITER_DEC`, `REV_BIT`), the same probability table layout and
   offsets, the same state transitions. Keep the C comments. Name things as
   the C does, in snake_case.
2. Same shape, same guarantees. `DecodeReal` runs only while
   `dicPos < limit` and `buf` has at least `LZMA_REQUIRED_INPUT_MAX` (20)
   bytes of margin. The wrapper (`LzmaDec_DecodeToDic`) enforces that and
   routes the tail through `LzmaDec_TryDummy` exactly as the C does. That
   margin is what makes unchecked loads sound; document it as the invariant
   on every `unsafe` block.
3. Unchecked where the C is unchecked, and nowhere else. Raw pointer or
   `get_unchecked` access is allowed inside the fast loop for `buf`, `probs`
   and `dic` under the margin invariant. Everything outside the loop is
   checked Rust.
4. Port, then measure, then optimize. Do not restructure before a faithful
   port passes differential tests and has a baseline number. Optimization
   means codegen: checking the assembly for bounds checks and spills, not
   redesigning the loop.
5. No `#[inline(never)]` splits of the hot loop, no trait-object dispatch in
   it, no per-bit function calls that the compiler may not inline.
6. Every step is checked against the oracle. See `docs/benchmarking.md`.

## Public API (initial)

```rust
pub struct LzmaProps { lc: u8, lp: u8, pb: u8, dict_size: u32 }
impl LzmaProps { pub fn parse(props: &[u8; 5]) -> Result<Self, Error>; }

pub struct LzmaDecoder { /* LzmaDec state + dictionary */ }
impl LzmaDecoder {
    pub fn new(props: LzmaProps) -> Result<Self, Error>;
    /// One call of LzmaDec_DecodeToBuf: consumes from `input`, writes to
    /// `output`, returns bytes read, bytes written and the status.
    pub fn decode(&mut self, input: &[u8], output: &mut [u8], finish: FinishMode)
        -> Result<Progress, Error>;
    pub fn reset(&mut self);
}

pub struct Lzma2Decoder { /* Lzma2Dec state over an LzmaDecoder */ }
impl Lzma2Decoder {
    pub fn new(dict_prop: u8) -> Result<Self, Error>;
    pub fn decode(&mut self, input: &[u8], output: &mut [u8], finish: FinishMode)
        -> Result<Progress, Error>;
}

pub enum Status { NotFinished, FinishedWithMark, NeedsMoreInput, MaybeFinishedWithoutMark }
pub enum Error { UnsupportedProps, CorruptData, /* … */ }
```

`std::io::Read` adapters (`LzmaReader`, `Lzma2Reader`) sit on top of that in
the `std` feature and are what a 7z/xz container parser consumes.

## Acceptance gate

Single-threaded decode wall time on `bench/fixtures/*.lzma` and the
`st.7z` pack stream, median of three, within 3% of `7zz t -mmt=1` on the
same file on the same idle machine. Until the port is at parity with the C
fast loop (`7lzma d`, no asm), the 3% number against `7zz` is not the
question; get to C parity first, then close on the asm.

## The container layer

The decoder is deliberately format-free: it decodes raw LZMA1 and raw LZMA2
and knows nothing about the files those streams arrive in. The layer above it
is not written yet; this section fixes its shape so that when it is, it is
ported from the same tree with the same discipline.

Two things that layer needs first, and which the crate already has: the
checksums in [`crate::crc`] (`crc` feature, `crc-fast`) and the cryptography
in [`crate::crypto`] (`crypto` / `aws-lc` features). Both are optional, off by
default, and unreachable from the decoder.

### xz

C: `C/Xz.h`, `C/XzIn.c`, `C/XzDec.c`.

A stream is a 12-byte header (magic `FD 37 7A 58 5A 00`, then two flag bytes
whose low nibble is the check type and whose CRC-32 covers the pair), a series
of blocks, an index, and a 12-byte footer. A block is a header naming one to
four filters — for this crate the only interesting chain is a single LZMA2
filter, id `0x21`, whose one property byte is the dictionary size — followed
by the filter output, padding to a multiple of four, and the check. The check
is CRC-32, CRC-64/XZ or SHA-256 depending on the stream flags, or absent.
`tests/common/mod.rs` already contains the minimum of this reader, in the
strictest possible form: it rejects everything it does not understand rather
than guessing, and the real one should keep that habit.

The index is what makes random access possible: it records, for every block,
the compressed and uncompressed size, so a reader can seek to a block boundary
without decoding what precedes it. The footer repeats the index size and its
CRC-32 so the index can be found from the end of the file.

### 7z

C: `C/7zIn.c` (header), `C/7zDec.c` (folder decoding), `C/7zAes.c`
(encryption), driven by `C/7zArcIn.c`.

A `.7z` file is a 32-byte signature header pointing at an encoded header at
the end of the file, which is itself often a compressed stream that must be
decoded before it can be parsed. The parsed header describes *folders*: a
folder is a small dataflow graph of coders (LZMA, LZMA2, BCJ, delta, AES, …)
with bind pairs joining outputs to inputs, packed streams feeding the leaves,
and one unpacked stream coming out. `7zDec.c`'s `SzFolder_Decode` is the
function to port, and its shape — decode the coder chain from the outside in,
one folder at a time, with the substream sizes and CRCs applied afterwards —
is the shape to keep.

Encryption is coder id `06F10701`: AES-256-CBC over the folder's packed
stream, with the key derived from the UTF-16LE password and the coder's salt
by the iterated SHA-256 in `7zAes.c`. [`crate::crypto::sevenz_key`] is that
derivation, and [`crate::crypto::Aes256Cbc`] is the unpadded CBC the folder
needs; the reader has only to feed them. Note the two special cycle counts
(`0x3F` means the salt and password *are* the key; `0x40` and above are
rejected), because they are easy to miss and impossible to guess from a
stream.

None of this is implemented. When it is: the same rules as the decoder — port
rather than redesign, cite the C function, prove it differentially against
`7zz x` and `xz -dc`, and keep it behind features so that a caller who only
wants raw LZMA still gets a crate with no dependencies.
