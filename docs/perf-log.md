# Decode throughput log

Every entry is a measurement of `lzma-fast` against the oracles on the same
machine in the same session, with the commit the number belongs to. See
[benchmarking.md](benchmarking.md) for the oracles and
[porting.md](porting.md) for the acceptance gate: single-threaded decode
within 3% of `7zz t -mmt=1`.

Machine: Apple M5 Max (arm64), 18 cores, macOS. `7zz` 26.01, `xz` 5.x, and
`7lzma` built from the reference tree at 0766b733 with its own `makefile`
(the portable C `LzmaDec_DecodeReal`, no assembly).

Fixtures (gitignored, see `bench/fixtures/README.md`): `p256.bin.lzma` is 256
MiB of synthetic payload at compression ratio 0.88, LZMA1, 8 MiB dictionary;
`payload.bin.lzma` is the same payload at 1 GiB; `p256.bin.xz` is the 256 MiB
payload as LZMA2 in an xz container.

## A note on the machine

This machine is shared with other build and test workloads, and the load
average during this session ranged from about 2 to 16 on 18 cores. Numbers
taken under load are pessimistic and noisy in one direction only: contention
can slow a run down but never speed it up. Two consequences, both applied
below.

- The harness reports the **median of N** runs per decoder, which is what the
  brief asks for; when the load moved between two decoders' blocks the median
  understates the faster one. Each table therefore also says what the load was
  doing.
- While iterating on the decoder itself, the figure used to accept or reject a
  change is the **best of 7** runs in one process, which is the run least
  disturbed by other work and the only statistic that is stable enough to
  resolve a 3% change on this machine.

## Baseline: the faithful C port

Commit `8265abd` (`feat(lzma): port the 7-Zip single-threaded LZMA and LZMA2
decoder`), `cargo run --release -p lzma-bench -- --runs 3`, load average
around 3.

| fixture | ours | `7lzma d` (C) | `7zz t -mmt=1` (asm) | `xz -T1` | vs 7lzma | vs 7zz |
| --- | --- | --- | --- | --- | --- | --- |
| `p256.bin.lzma` | 6.393 s | 6.299 s | 4.150 s | 6.172 s | 1.015 | **1.540** |
| `payload.bin.lzma` | 33.078 s | 24.195 s | 26.197 s | 31.197 s | 1.367 | **1.263** |
| `p256.bin.xz` (LZMA2) | 6.036 s | n/a | 3.894 s | 6.376 s | n/a | **1.550** |

Reading: the port is at parity with the reference C decoder on the 256 MiB
LZMA1 fixture (1.5% behind), which is the first milestone in the brief. It is
half again slower than `7zz`, and `7zz` is exactly the same C decoder with
`Z7_LZMA_DEC_OPT` on, i.e. with `Asm/arm64/LzmaDecOpt.S` substituted for
`LzmaDec_DecodeReal_3`. The whole remaining gap is that assembly loop.

The `payload.bin.lzma` row is the one contended measurement in this table: a
stray debug test binary from an earlier session was pinned to a core for its
whole duration (it could not be killed from this session), and four `rustc`
processes started during it. Decoding only the first 256 MiB of
`payload.bin.lzma` runs at the same rate as `p256.bin.lzma`, which is the same
data, so the 1.37 figure is measurement noise rather than a size effect.

## Step 1: branchless bit decoding in the portable loop

Best of 7 on `p256.bin.lzma`: **42.4 -> 50.1 MiB/s**, a 18% gain, and the
first change that moves the portable path past the reference C decoder.

`LzmaDec_DecodeReal`'s `IF_BIT_0` / `UPDATE_0` / `UPDATE_1` is a branch on a
range-coder bit, which is close to a coin flip and mispredicts about half the
time — eight times per literal, and literals are almost all of this payload.
`Asm/arm64/LzmaDecOpt.S` does not branch there: `NORM_CALC` + `CMOV_range` +
`CMOV_code_Model_Pre` + `PUP_BASE_2` decode the bit with conditional selects,
and `BIT_1_R` loads *both* children of the tree node before the select, so the
next level's load latency overlaps this level's arithmetic.

The portable loop now has the same shape, in Rust:

- one `kBitModelOffset` expression serves as both `UPDATE_0` and `UPDATE_1`
  (`ttt - (t >> kNumMoveBits)` with `t = bit ? ttt : ttt - kBitModelOffset`);
- `range`, `code`, the symbol and the next probability are selected with
  arithmetic masks, not `if`;
- the literal, length, pos-slot and align trees prefetch both children.

Two things had to be measured rather than assumed. Writing the selects as
`if cond { a } else { b }` is *slower than the original branchy code* (36.9
MiB/s): LLVM re-forms a conditional branch out of arms this cheap, so it pays
the mispredict and the extra work. Written as mask arithmetic, LLVM emits
`csel` and the branch is gone. Selecting the prefetched child as a `u16` or
with an `if` also lost 2-5 MiB/s against the `u32` mask form.

The length, pos-slot, align and reverse-bit trees got the same treatment in
the same step; on their own they were worth nothing measurable on this payload
(50.1 vs 48.1 MiB/s is inside the noise), because a payload that compresses to
0.88 is nearly all literals. They are kept for shape consistency and for
match-heavy inputs.

## Step 2: the SDK's own assembly loop

Commit `da2e5dd` (`feat(lzma): decode with the SDK's hand-written aarch64 and
x86_64 loops`),
`cargo run --release -p lzma-bench -- --runs 5`, load average around 3.

The harness changed with this step: it now **interleaves** decoders, running
one timed run of each per round instead of all N runs of one decoder before
moving to the next. On a shared machine a block schedule gives whichever
decoder happened to run during a quiet minute an advantage that no number of
repetitions removes, because the median is taken within the block. Round-robin
spreads the same load over every decoder. This is why the `7zz` column moves
between the tables below and the ones above.

| fixture | ours (asm) | ours (portable) | `7lzma d` (C) | `7zz t -mmt=1` (asm) | `xz -T1` | asm vs 7zz |
| --- | --- | --- | --- | --- | --- | --- |
| `p256.bin.lzma` | 3.746 s | 5.126 s | 5.680 s | 3.771 s | 5.868 s | **0.993** |
| `payload.bin.lzma` | 15.248 s | 20.376 s | 22.726 s | 15.032 s | 23.339 s | **1.014** |
| `p256.bin.xz` (LZMA2) | 3.748 s | 5.109 s | n/a | 3.834 s | 6.059 s | **0.978** |

That is the acceptance gate: 0.978, 0.993 and 1.014 against `7zz t -mmt=1`,
all inside 3%, two of the three faster than 7-Zip. The portable loop is at
0.90 of the reference C decoder and 1.36 of `7zz`; the difference between the
two columns is entirely the inner loop.

Whether the arm64 loop could be beaten on this machine was asked separately
and answered no: see [arm64-tuning.md](arm64-tuning.md), which measures five
structural rewrites of the loop (all lost or were noise), and derives the
~6.3-cycle loop-carried recurrence the stock loop already runs at. That is why
the aarch64 module here is a faithful translation of the stock `.S` and not a
tuned variant, and why "do not add an instruction to the hot path" is the rule
the translation was checked against.

### What was ported

`Asm/arm64/LzmaDecOpt.S` and `Asm/x86/LzmaDecOpt.asm` (the latter with
`Asm/x86/7zAsm.asm` spliced in), translated line by line into
`src/lzma/decode_opt/lzma_dec_opt_aarch64.S`,
`lzma_dec_opt_x86_64_sysv.S` and `lzma_dec_opt_x86_64_win64.S`, each included
verbatim into a `#[unsafe(naked)]` Rust function by
`decode_opt/{aarch64,x86_64}.rs`. A naked function is the right vehicle: these
are whole C-ABI functions with their own prologue, frame and callee-saved
register handling, so there is nothing for the register allocator to do and
nothing to declare but the symbol.

The assembly reads `CLzmaDec` through hardcoded offsets, so `AsmLzmaDec` is a
`#[repr(C)]` mirror of it whose every offset and whose total size are asserted
at compile time with `offset_of!`. Nothing about the Rust state layout can
drift away from the assembly without failing the build.

Three mechanical differences from the reference files, listed in each file's
header and nowhere else:

- the C preprocessor and the MASM macro assembler are resolved by hand, for
  the same configuration the reference builds use (16-bit probabilities,
  `LZMA_USE_4BYTES_FILL` on, `_LZMA_SIZE_OPT` off);
- every label gains an `L` prefix, because Mach-O refuses a conditional branch
  to a label that is not assembler-local;
- on x86, every constant expression is folded to a plain integer.

That last one is not cosmetic. LLVM's GNU-Intel parser silently drops the
index register of a memory operand whose scale is parenthesised:
`[probs + sym * (2)]` assembles as `[probs]`, with no diagnostic. The first
translation had that in every literal tree load, which is exactly the sort of
bug a differential test catches and a reading does not.

### How the translation was checked

Beyond the differential tests, the x86 translation was checked against the
reference at the instruction level. `llvm-ml64` will assemble MASM, so the
reference file was assembled (after removing four textual `equ`s it does not
support, which is a smaller change than the translation itself) and both
objects disassembled and diffed. They agree instruction for instruction; the
only differences are alignment padding, and one instruction the *oracle* was
missing because of a flaw in its own preparation.

`tests/asm_parity.rs` then decodes every committed vector, every truncation
prefix of every vector, and 400 random single-bit corruptions of each, with
both loops, and requires identical bytes and identical errors. The full suite
passes with the `asm` feature, with `--no-default-features --features std`,
and as `no_std` + `alloc`, on aarch64 macOS, on x86_64 macOS under Rosetta and
on x86_64 Linux.

## The same loop on linux-x86_64 (x86-box)

Commit `71d8ced`, the same harness, `--runs 3`, on the x86_64 bench box:
`12th Gen Intel(R) Core(TM) i5-1240P`, 16 logical CPUs, Ubuntu 26.04.1, gcc
15.2.0, `xz` 5.8.3. Everything below is pinned with `taskset -c 0-5` — ours
and every oracle alike — and the box was idle (load average under 1) before
each series. Fixtures were regenerated on the box, so their CRCs differ from
the macOS tables above; the payloads are generated by the same script to the
same parameters.

**Hybrid-core caveat.** This is an Alder Lake-P part: four P-cores with SMT
(CPUs 0-7, 4.4 GHz) followed by eight E-cores (CPUs 8-15, 3.3 GHz).
`taskset -c 0-5` pins every decoder, ours and the oracles alike, to the
threads of the first three P-cores, which is the only way a single-threaded
number on this machine means anything: an unpinned run can be migrated onto an
E-core mid-decode and lose a large fraction of its throughput for reasons that
have nothing to do with the code. It also means every figure here is a P-core
figure and says nothing about what the same binary does on an E-core. Because
the pinning is identical for all six decoders, the ratios are unaffected by
the choice; only the absolute MiB/s would move.

**No distro `7zz`.** The box has no 7-Zip package, so the oracle was built
from the reference tree (shallow clone of `ip7z/7zip` at `0766b733`) with
`make -f makefile.gcc`. That build has **no assembly loop**: there is no
`LzmaDecOpt.o` in its link line, so a stock `makefile.gcc` `7zz` on Linux is
the C decoder and is not the thing the gate is stated against. The gate
therefore uses a second binary, `bin-asm/7zz`, built from the same tree with
`-DZ7_LZMA_DEC_OPT` and `Asm/x86/LzmaDecOpt.asm` assembled in — 7-Zip's own
fast path, which is what `7zz` ships as on x86_64 elsewhere. `7lzma` is the
tree's `C/Util/Lzma` build, C only.

| fixture | ours (asm) | ours (portable) | `7lzma d` (C) | `7zz t -mmt=1` (asm) | `xz -T1` | asm vs 7zz |
| --- | --- | --- | --- | --- | --- | --- |
| `p256.bin.lzma` | 4.865 s | 5.844 s | 10.344 s | 4.897 s | 5.106 s | **0.994** |
| `payload.bin.lzma` | 19.514 s | 23.383 s | 41.177 s | 19.596 s | 20.458 s | **0.996** |
| `p256.bin.xz` (LZMA2) | 4.884 s | 5.860 s | n/a | 5.007 s | 5.127 s | **0.976** |

The gate is met on x86_64 as it is on aarch64: 0.976, 0.994 and 0.996, all
inside 3% and all on the fast side of it. The portable loop is at 1.19 of
`7zz`, the same order as its 1.36 on aarch64.

One number here is not like its macOS counterpart. `7lzma` — the same C source
the portable loop is a port of — runs at 24.9 MiB/s here against 45 MiB/s on
the Mac, so our portable loop is 0.57 of it rather than 0.90. `xz` is *faster*
here than on the Mac (50 vs 43 MiB/s), so this is not a slow machine; it is
gcc 15 compiling that inner loop about twice as badly as clang does. It is
worth recording because it makes "within 3% of 7-Zip" and "2x the reference C
decoder" the same sentence on this box, and only the first of those is a claim
about this crate.
