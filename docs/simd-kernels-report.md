# Word-at-a-time kernels: what was measured, what shipped

Seven candidates were profiled and, where the profile justified it, written and
measured. Three shipped. Four did not, and their numbers are here too, because
a kernel that was measured and lost is a more useful record than one that was
never tried.

Everything below is 0.4.0 on `feature/simd-kernels`, plus the `sevenz-turbo`
branch of the same name.

## The bar

A kernel was kept only if all four held:

1. **Bit-exact.** A differential test holds it against the loop it replaced
   over an adversarial corpus - boundaries, every distance and mask, chunk
   splits, state carried across calls - and, where the crate has one, the SDK
   oracle stays the authority.
2. A microbenchmark win on the tier's own ISA.
3. **At least 2% end to end** on a host where the tier engages, from an
   interleaved A/B - base, cand, base, cand - of at least five rounds, medians
   compared, *one binary* with the arm chosen by a cached environment toggle
   sitting exactly where a cached target-feature probe would sit. No rebuild
   between arms.
4. **No lane regresses by more than 1%** on any host.

Anything else was dropped and its code removed.

## Hosts

| Name | CPU | Notes |
| --- | --- | --- |
| i5-1240P | Alder Lake-P, AVX2/BMI2/VAES/VPCLMUL/GFNI, no AVX-512 | Linux. Runs pinned to the P-cores, `taskset -c 0-5`. Quiet while used. |
| Zen 2 | AMD, AVX2/BMI2, no AVX-512, no VPCLMULQDQ | Windows. Unpinned; no `start /affinity`. Very quiet: lane-to-lane spread under 0.3%. Wall clock only - CPU time is not reliable there. |
| M5 Max | aarch64, NEON | Contended throughout by unrelated CI. Indicative only, and said so wherever it is quoted. |

Part-way through, the i5-1240P was taken for other work and the Zen 2 box
replaced it. Numbers already taken on the i5-1240P stand; everything after that
point is Zen 2. Both are named per row below.

No AWS instance was rented, and no instance-hours were spent. Nothing that
survived is ISA-specific: every kernel here is portable SWAR over a `u64`, so
there was no AVX-512 or SVE2 tier whose measurement needed silicon that is not
on this bench. Had one been written, the fleet would have been.

## The toggles

`kernel-ab` is a non-default feature, implying `std`, `enc` and `xz`, which
compiles a cached branch in front of each kernel. Off - which is every shipped
build - there is no branch and no environment lookup. The toggles are
`LZMA_TURBO_MATCH_RUN` (`scalar`, `w8`, `w16`), `LZMA_TURBO_BCJ_SCAN`
(`scalar`, `wide`) and `LZMA_TURBO_DELTA` (`scalar`, `block`). See
`src/kernel_ab.rs`.

Two builds cannot be compared at the couple of percent this gate is written in:
inlining, code layout and placement move by more than that for reasons that
have nothing to do with the kernel. Hence one binary.

One trap worth recording, because it silently produces a null result: in
`cmd.exe`, `set VAR=value && prog` puts the space before the `&&` *into the
value*. The first Zen 2 run of candidate 1 read +0.10%/+0.00%/+0.24% because
both arms were getting an unrecognised value and falling through to the
default. `set "VAR=value"` is the fix, and the rerun is the row below.

## Profile

Profiled with `samply`/`sample` on the M5 Max; `perf` on the i5-1240P is
unavailable to an unprivileged user and the setting was not changed.

The repository's own `p256.bin` fixture is deliberately match-poor - 70% short
hex words from a 4000-word dictionary, 30% random runs - and it *understates*
the match-extension loops at about 3.5% of the encode. A corpus of real source
and executables (`mixed.bin`, 39.7 MiB) puts the same loops at 21%. Both are
recorded because the difference is the story: a fixture chosen to stress the
decoder says nothing about where the encoder spends its time.

Filter ceilings were measured rather than guessed, by decoding the same bytes
with and without the filter in the chain:

| Filter | Cost of the filter, as a share of the lane |
| --- | --- |
| Delta, distance 4 | 6.1% |
| BCJ x86, over x86 code | 10.0% |
| BCJ ARM64, over aarch64 code | 4.0% |

## 1. Match extension - KEPT

`UPDATE_maxLen`, `GetMatchesSpec1`, `SkipMatchesSpec` and `Hc_GetMatchesSpec`
(`LzFind.c`) and `GetMatchesSpecN_2` (`LzFindOpt.c`) all ask one question with
five different sets of index arithmetic: from here, how far do these bytes and
the bytes `distance` behind them agree, up to a limit? All five asked it a byte
at a time. `src/enc/match_run.rs` asks it eight bytes at a time - the first
differing byte in a 64-bit XOR is the one `trailing_zeros` names, once both
words are read little-endian, so the byte index does not depend on host byte
order.

Codegen was checked. The first version indexed the two slices and kept a bounds
check the compiler could not discharge - an extra `add`/`cmp`/`b.hs` per
iteration, visible in `--emit asm`. Walking `chunks_exact(8)` pairs instead left
two loads, a compare and a branch.

**Bit-exactness.** `match_run`'s own tests hold it against the byte loop for
every `(distance, start, limit)` over windows whose first difference lands on
every offset in a word and on both sides of every word boundary, plus constant,
overlapping and pseudo-random windows. The SDK oracle is unmoved: `lzma_parity`,
`mt_match_finder` and `lzma2_mt_parity` all pass, including the threaded finder
against the SDK built without `Z7_ST`.

**Microbenchmark** (scan alone, against the byte loop):

| Host | byte loop | 8 bytes | 16 bytes |
| --- | --- | --- | --- |
| i5-1240P | 1.00x | 2.46x | 2.98x |
| M5 Max | 1.00x | 2.68x | 3.40x |

**End to end**, scalar against the eight-byte scan, medians of seven
interleaved rounds:

| Host | Lane | base | cand | delta |
| --- | --- | --- | --- | --- |
| i5-1240P | mixed p6 | 8.276s | 7.178s | **+13.27%** |
| i5-1240P | mixed p9 | 10.764s | 9.291s | **+13.68%** |
| i5-1240P | mixed p6, 2 match-finder threads | 4.937s | 4.165s | **+15.64%** |
| i5-1240P | mixed p1 | 1.033s | 1.016s | +1.65% |
| i5-1240P | rngpayload p6 (no long matches) | 23.836s | 23.800s | +0.15% |
| Zen 2 | mixed p6 | 8.745s | 7.815s | **+10.63%** |
| Zen 2 | mixed p9 | 11.036s | 9.671s | **+12.37%** |
| Zen 2 | mixed p6, 2 match-finder threads | 4.804s | 4.201s | **+12.55%** |

Zen 2 rounds, p6: base 8.772 8.770 8.732 8.732 8.850 8.745 8.729; cand 7.848
7.808 7.833 7.815 7.777 7.803 7.824.

Clause 4: the worst lane is the one with no long matches to skip, at +0.15%.
Nothing regresses.

**Sixteen bytes lost.** It wins the scan alone on both hosts and loses the
encode, because the runs the finder walks are mostly shorter than one of its
words and what it wins on the long ones does not pay for its tail on the short
ones:

| Host | Lane | w8 | w16 | delta |
| --- | --- | --- | --- | --- |
| i5-1240P | mixed p6 | 7.331s | 7.419s | -1.20% |
| i5-1240P | mixed p9 | 9.369s | 9.439s | -0.75% |

`run16` stays in the tree behind `kernel-ab` and under test, because the number
above is only worth anything if the arm that produced it was correct.

## 2. BCJ x86 branch search - KEPT

The one part of `Z7_BRANCH_CONV_ST(X86)` that is a search rather than a state
machine is the run from one `E8`/`E9` to the next. On code the filter was not
built for - the wrong architecture, a data section - that run is nearly the
whole cost, because there is nothing to convert and every byte is still read.

`0xE8` and `0xE9` differ only in bit 0, so setting bit 0 of every byte maps both
onto one value and nothing else onto it. "Is one of two values" becomes "is one
value", which is the classic zero-byte test, whose lowest set flag is at the
lowest zero byte - the one the C would have reached first.

**Bit-exactness.** Three tests. `find_branch_byte` against the byte scan for
every length and alignment over an alphabet built from the bytes a zero-byte
test gets wrong (`0x00`, `0x80`, `0xE7`, `0xE8`, `0xE9`, `0xEA`). The whole
converter against a copy of the four-byte loop it replaced - conversions,
return values *and* carried state - whole, and fed in pieces at ten chunk
sizes. The subtle part is not the search: the C stops at the end of a four-byte
stride, which can be up to three bytes past the limit, and reports that as
converted. Mutating `stride_end` to `lim` fails both converter tests, so they
bite. `filter_parity` against the SDK's own `filter-oracle` passes.

**End to end**, i5-1240P, medians of seven interleaved rounds, decoding 64.0 MiB
of x86 executables:

| Lane | base | cand | delta |
| --- | --- | --- | --- |
| through the x86 filter | 0.305s | 0.297s | **+2.62%** |
| same bytes, no filter (control) | 0.286s | 0.286s | +0.00% |

Rounds, filtered: base 0.303 0.306 0.308 0.304 0.302 0.305 0.306; cand 0.296
0.297 0.295 0.303 0.297 0.301 0.301.

The control is the important row: the toggle cannot reach a lane with no filter
in it, and it reads exactly zero, which is what says the rig is measuring the
kernel and not the weather.

Corroborated on Zen 2 through a different harness - see candidate 7, where the
same filter reads +5.96% inside a 7z extract.

## 3. BCJ ARM64 instruction scan - DROPPED

Written, measured, and removed. In 64 MiB of real aarch64 code, 1.27% of
instructions are `BL` and 0.56% are `ADRP`, so 98% of the converter's work is
deciding there is no work - which looked like a good prefilter. Both tests read
only the high byte of the little-endian word (`b & 0xFC == 0x94` and
`b & 0x9F == 0x90`), so masking a pair of instructions and filling the other
three bytes of each turns them into one zero-byte test.

It was correct - a differential test against the two tests for every high byte
at every alignment, a whole-converter test against the previous loop at four
`pc` values in both directions, and the SDK oracle, with a mutated `ADRP_MASK`
failing both - and it was faster. It was not faster **enough**:

| Host | Lane | rounds | base | cand | delta |
| --- | --- | --- | --- | --- | --- |
| i5-1240P | through the ARM64 filter | 7 | 0.305s | 0.297s | +2.33% |
| i5-1240P | through the ARM64 filter | 15 | 0.340s | 0.336s | **+1.18%** |
| i5-1240P | no filter (control) | 15 | 0.318s | 0.320s | -0.63% |
| M5 Max (contended) | through the ARM64 filter | 11 | 0.263s | 0.258s | +1.90% |
| M5 Max (contended) | no filter (control) | 11 | 0.248s | 0.248s | +0.00% |

**Gate clause 3.** Seven rounds read +2.33% with a control that had drifted
+0.92% in the same session - a signal barely above its own noise floor. Fifteen
rounds settled it at +1.18%, which does not clear 2%, and the aarch64 host, for
what a contended host is worth, agreed at +1.90%. The filter is only 4.0% of
that lane to begin with, so even removing it entirely would have left little
room.

The code is gone. It is a real win, just a smaller one than the bar.

**ARM, ARM Thumb, PowerPC, SPARC: not measured.** No 32-bit ARM, PowerPC or
SPARC corpus is on this bench, and generating a synthetic one measures the
generator. They are the same shape as ARM64 - walk four-byte words, test a
mask, convert rarely - with a *cheaper* per-instruction test than ARM64's two,
so the share a prefilter could address is smaller and the ceiling lower. ARM64
was the most favourable member of the family and came in at 1.18%. Writing the
others would have been writing to lose.

## 4. Delta filter - KEPT

Not a SIMD kernel so much as a shape fix. `data[i] += data[i - delta]` only
reaches back `delta` bytes, so any `delta` consecutive outputs depend on bytes
that are already final and on nothing inside their own block. Adding a block at
a time says exactly that, as two slices the compiler can see cannot overlap,
and the add vectorizes. Below 16 the block is shorter than a vector register
and the per-block bookkeeping costs more than the byte loop, so the byte loop
stays.

**Bit-exactness.** The existing tests cover distances 1, 2, 3, 4, 16, 255 and
256 with chunk splits, and a differential against the spec's own reference
encoder (§5.3.3.1). `filter_parity` against the SDK's `filter-oracle` passes.

**End to end**, i5-1240P, medians of seven interleaved rounds:

| Lane | base | cand | delta |
| --- | --- | --- | --- |
| distance 64 | 0.424s | 0.411s | **+3.07%** |
| distance 4 (byte loop still) | 0.279s | 0.278s | +0.36% |

Rounds, distance 64: base 0.427 0.421 0.420 0.424 0.427 0.434 0.421; cand 0.411
0.411 0.413 0.411 0.411 0.409 0.407.

Clause 4: the narrow distance, which takes the unchanged path, moves +0.36%.

## 5. `normalize3` - DROPPED on codegen, not on a timer

The SDK has SSE2 and AVX2 `LzFind_SaturSub_128/256` for this. The baseline
already auto-vectorizes: `--emit asm` shows `pmaxud` + `psubd` on x86-64 and
paired `ldp q` on aarch64. There is no instruction-level gap to close, so no
kernel was written and nothing was timed. This is what "verify the baseline
does not already auto-vectorize before writing a kernel" is for.

## 6. Threaded match finder hash thread - DROPPED on the ceiling

Nothing was written. A sample of the `--mf-threads 2` encode shows the hash
thread 847 samples out of 909 - **93%** - blocked in `Semaphore::wait`, while
the bt thread runs 906 out of 909. The hash thread is not the bottleneck and
its inner loop cannot move wall time; the ceiling is effectively zero. The
handshake, not the hashing, is what would have to change, and that is a
different piece of work.

## 7. sevenz-turbo: one copy of the filters - KEPT, with a caveat

`sevenz-turbo` decodes LZMA and LZMA2 with this crate and *also* carried its
own port of the same eight branch converters and the same delta filter, both
vendored from `lzma-rust2` 0.20.1, both from the same public-domain C this
crate ports. On branch `feature/simd-kernels` there, `src/codec/filter/bcj/` is
deleted and `BcjFilter` and `Delta` are handles on `lzma_turbo::filters::{bcj,
delta}`. The vendored readers and writers are untouched, so that crate's API
does not move. BCJ2 stays vendored: .xz has no BCJ2, so this crate has none.

Its delta filter was the 256-byte ring the brief flagged - a moving index, and
per byte in both directions two masked index computations, a load, an add and a
store. It inherits this crate's shape instead.

**This one is two builds, not two arms.** The change is the deletion of a
duplicate implementation, so there is no arm to toggle: keeping both to toggle
between would be keeping the thing being removed. Gate clause 3's one-binary
requirement cannot apply, and the numbers below are reported as what they are.
They are supported by the clause-3 measurement of the same kernels inside this
crate, above.

**End to end**, Zen 2, one thread, medians of nine interleaved rounds:

| Archive | base | cand | delta |
| --- | --- | --- | --- |
| delta, distance 64, 39.7 MiB | 0.456s | 0.414s | **+9.21%** |
| delta, distance 4, 39.7 MiB | 0.313s | 0.290s | **+7.35%** |
| BCJ x86, 64.0 MiB | 0.386s | 0.363s | **+5.96%** |
| BCJ ARM64, 64.0 MiB | 0.374s | 0.373s | +0.27% |
| no filter (control) | 0.345s | 0.345s | +0.00% |

Spread on that host is under 0.3% lane to lane, and the control is flat.

The M5 Max was asked the same question first and could not answer it: load
average above 9 from unrelated CI, and the ARM64 lane read **-2.27%** with a
control reading -0.73%. The Zen 2 rerun shows that was noise. It is recorded
because a number taken on a contended host is worth exactly nothing, and the
temptation to report the first one is the thing to resist.

**Behaviour.** The whole `sevenz-turbo` suite passes with the swap in place -
every lane, including the round-trip compression tests that exercise each BCJ
filter in both directions.

**Caveat, and it is a real one.** That branch cannot build from crates.io:
`lzma-turbo` 0.4.0 is not published. Building it needs a `[patch.crates-io]`
pointing at a checkout of this worktree, and its `Cargo.lock` carries the
patched `lzma-turbo` entry with no source and no checksum. When 0.4.0 ships,
that lock needs one refresh. It also turns on this crate's `xz` feature, which
is where the filters live, so it now compiles the .xz container layer it does
not otherwise use.

## Fixtures

Branch filters only do anything to machine code, and no binary is committed.
`cargo xtask fixtures` grows four entries that build it from what a machine
already has: `codebin.bin` is the executables under `target/`, largest first,
up to 64 MiB - real code, for whatever architecture the machine is - and
`bcj-x86.code.xz` and `bcj-arm64.code.xz` are that through each converter, so
one of the two is always the case that matters most, a filter run over code it
was not built for. `delta64.xz` is the delta filter at a distance wide enough
to add a block at a time, next to the existing `delta.xz` at distance 4.

The Zen 2 and i5-1240P corpora were built the same way from those hosts' own
ELF binaries, so the x86 lanes are real x86 code rather than a filter run over
something else.

## Summary

| # | Candidate | Verdict | The line that decided it |
| --- | --- | --- | --- |
| 1 | Match extension, 8 bytes | **KEPT** | +13.27% / +13.68% / +15.64% (i5-1240P), +10.63% / +12.37% / +12.55% (Zen 2); worst lane +0.15% |
| 1b | Match extension, 16 bytes | DROPPED | -1.20% end to end, despite 2.98x in the microbenchmark |
| 2 | BCJ x86 branch search | **KEPT** | +2.62%, control +0.00% |
| 3 | BCJ ARM64 instruction scan | DROPPED | +1.18% at 15 rounds; clause 3 wants 2% |
| 3b | BCJ ARM / Thumb / PPC / SPARC | not measured | no corpus; cheaper test than ARM64's, so a lower ceiling than the 1.18% that already failed |
| 4 | Delta, block at a time | **KEPT** | +3.07% at distance 64, +0.36% at distance 4 |
| 5 | `normalize3` | DROPPED | baseline already emits `pmaxud`/`psubd` and `ldp q` |
| 6 | Hash thread | DROPPED | 93% blocked on a semaphore; ceiling ~0 |
| 7 | sevenz-turbo filter swap | **KEPT** | +9.21% / +7.35% / +5.96%, control +0.00%; two builds, not two arms |

AWS instance-hours: **0**.
