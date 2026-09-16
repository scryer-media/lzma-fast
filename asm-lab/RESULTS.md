# arm64 LZMA decode-loop tuning on Apple M5 Max — full experiment log

Target: 7-Zip 26.03 `Asm/arm64/LzmaDecOpt.S` (`LzmaDec_DecodeReal_3`), measured
against the shipped Homebrew `7zz` 26.01 (arm64, ASM) on macOS.

**Headline: no variant beat stock by more than the noise floor.** Five
structural rewrites were built, verified byte-exact and measured; four were
slower, one tied. The reason is a specific and repeatedly-confirmed property of
this loop on this core, documented under "Performance model" below. The shipped
`LzmaDecOpt-m5.S` is therefore code-identical to stock.

---

## 1. Harness

| file | what it does |
|---|---|
| `build.sh` | builds the SDK `7lzma` CLI against a chosen decode loop (`./build.sh c`, `./build.sh LzmaDecOpt-stock.S`, `./build.sh variants/NN-x.S`, `PROB32=1 ./build.sh ...`) |
| `bench.py` | min-of-N **child user+sys CPU time** |
| `verify.sh` | byte-exact decode of both fixtures plus 63 small `xz --format=lzma` streams across presets and lc/lp/pb |
| `mkvariant.py` | generates a variant by exact-match string edits on the stock source; a missed or ambiguous edit is a hard error, so a variant can never silently be a copy of stock |
| `hotlabels.py` | attributes `samply` samples to asm basic blocks by raw module-relative address + `objdump` branch targets (samply will not symbolicate this static binary) |
| `chainbench.c` | isolates the loop-carried recurrence latency of candidate range-decoder formulations |

### Measurement caveats (important)

* **Hardware CPU counters were unavailable.** `xctrace` needs a full Xcode
  install (this box has Command Line Tools only) and `instruments` is not
  present. So there are **no measured IPC or branch-mispredict counters
  anywhere in this report** — every microarchitectural claim below is inferred
  from wall/CPU time, from `chainbench` latency microbenchmarks, and from
  in-situ "add N instructions and see" diagnostics. Where I say "throughput
  bound" or "the branch predicts well", that is an inference, not a counter
  reading.
* The machine was **shared with another agent** doing heavy Rust builds for
  most of the session (load average peaked at 24 on 18 cores). `bench.py` was
  switched mid-session from median wall time to **min-of-N CPU time** for this
  reason. On a quiet box the harness repeats to about ±0.5%; under load, ±3%.
  Every number quoted as a conclusion below was re-taken on a quiet box.
* Numbers marked *(loaded)* were taken during contention and are indicative
  only.

---

## 2. Performance model (the thing that explains every result)

Measured on the literal-heavy fixtures: **~59.9 cycles per output byte**, and
the streams need ~9 binary decodes per byte, so **~6.65 cycles per binary
decode**. `objdump` shows the unrolled literal tree at **20 instructions per
bit** (17 of which issue in the common case; the 3-instruction refill body is
branched over ~7 times out of 8).

Two independent limits sit almost exactly on top of each other:

1. **Recurrence latency.** `chainbench` measures the stock loop-carried chain
   `csel(range) -> lsr#11 -> mul -> subs` at **6.30 cycles/iteration** in
   isolation (clock measured at 4.21–4.24 GHz via an `add` chain). An
   alternative `mul`+`msub` formulation with a speculative shift measures
   **5.53**.
2. **Issue throughput.** In-situ diagnostics `d4-plus1insn` / `d5-plus3insn`
   insert 1 and 3 genuinely independent, dead-result `eor` instructions at the
   top of `BIT_1_R`. One extra instruction costs **+3.7%** (~0.32 cycles).

6.30 (latency) vs 6.65 (actual) leaves ~0.35 cycles of slack. That is why:

* **adding** instructions costs ~0.32 cyc each immediately (nothing absorbs
  them), and
* **removing** one instruction buys nothing, because the 6.30-cycle chain then
  becomes the binding constraint.

So a win requires shortening the chain **and** not growing the instruction
count. Every formulation I could find that shortens the chain needs strictly
more instructions to do it (see 01/02), and the one formulation that removes an
instruction (03) is only expressible in a way that introduces a store-forwarding
hazard. This is a genuinely tight local optimum, not an oversight by Pavlov.

### ARM64 addressing constraints that close the obvious doors

The tree walk wants `child = probs[2*sym + bit]`, and the update wants to store
at `probs[sym]`. The scaled-register addressing modes only allow a shift of 0
or `log2(access size)`:

| load | legal shifts |
|---|---|
| `ldrh` (16-bit probs) | 0, 1 |
| `ldr w` (32-bit probs) | 0, 2 |
| `ldr x` (64-bit pair)  | 0, 3 |

* With 16-bit probs, addressing the children from the **undoubled** `sym` needs
  `lsl #2` on an `ldrh` — **illegal**. Hence stock's `add sym,sym`.
* With 32-bit probs, the children sit at byte `8*sym` and `8*sym+4`, needing
  `lsl #3` on an `ldr w` — **illegal**. This killed variant `04` at the
  assembler, and is the reason the only way to exploit 32-bit probs is a 64-bit
  pair load (variant `03`).
* `ldp` has no register-indexed form at all, which kills the "load four
  grandchildren and decode two tree levels per addressing op" idea outright.

---

## 3. Results

All figures: min-of-N CPU seconds decoding the 32 MiB slice of `p256.bin`,
quiet box, N=13. Stock was measured at both ends of the sweep (0.4807 /
0.4767) which sets the run-to-run spread at ~0.8%.

| variant | s (cpu) | vs stock | verdict |
|---|---|---|---|
| **stock** | **0.4767–0.4807** | — | baseline |
| `03-pair32` (+PROB32) | 0.4812 | +0.5% | **no consistent sign** (see below) |
| `01-msub-rangeA` | 0.4891 | +2.2% | slower |
| `02-msub-cand` | 0.4922 | +2.8% | slower |
| `05-normless` | 0.5220 | +9.1% | much slower |

Copy-loop build flags, measured on a match-heavy stream (7-Zip source tar,
15.9 MiB out, ratio 0.26, decodes at ~182 MiB/s — 2.8x the literal-heavy rate):

| variant | s (cpu) | verdict |
|---|---|---|
| stock (`LZMA_USE_4BYTES_FILL` on) | 0.0855–0.0872 | baseline |
| `06-copy2b` (`LZMA_USE_2BYTES_COPY`) | 0.0857 | no effect |
| `07-cmovwrap` (`LZMA_USE_CMOV_LZ_WRAP`) | 0.0865 | no effect |
| `08-nofill4` (4-byte fill off) | 0.0874 | no effect |
| `09-copy2b-cmov` (both) | 0.0912 | marginally worse |

All five are inside the noise band for a 0.086 s run; none is worth taking.

---

## 4. Every variant, and why it lost

### `01-msub-rangeA` — carry `range>>11`, use `msub` for the remainder (+2.2%)

Keeps `rangeA = range >> 11` live in `t0` so the `lsr` leaves the critical path,
and computes `bound`/`rem` as `mul` + `msub` (the `msub` addend→result latency
is **1 cycle** on M5, confirmed by `chainbench`'s `k_muladd_lat`). Isolated
chain: 5.53 vs 6.30 cycles.

Costs **+2 instructions per bit**: selecting the new `rangeA` needs `lsr` from
*both* candidates plus a second `csel` (`SEL_RANGE_A`), where stock needs one
`csel` plus one `lsr`. Predicted +0.66 cyc against a −0.77 cyc chain saving,
i.e. roughly a wash; measured a clear loss. **This is the single most important
negative result: the 0.77-cycle chain saving does not materialise in situ**,
which says the loop is issue/throughput-limited rather than purely
latency-limited, and retires the whole "shorten the recurrence" family.

*Bug found and fixed during development:* `len8_loop` jumps straight into
`BIT_1` without a preceding `BIT_0`, so the newly-carried `rangeA` was never
seeded, producing `Error: Data error` only on streams containing a long match.
Bisected with the `rep` corpus. Any port that adds loop-carried state must seed
it at **every** entry into the bit macros, not just the obvious one.

### `02-msub-cand` — 01 plus precomputed child offsets (+2.8%)

On top of 01, precomputes both candidate child byte-offsets (`lsl`, `add`)
before the flags are known and `csel`s between them. One more instruction than
01; lost by exactly that much. Also had to spill `bufLimit` (x16) and
`checkDicSize` (x27) to the stack to free registers — reloaded in `CheckLimits`
and `decode_dist_end`. **Register pressure is at the limit**: the loop already
uses x0–x17 and x19–x30, with x18 reserved on Apple platforms. Any scheme
needing two more live values pays for a spill.

### `03-pair32` — 32-bit probs, one 64-bit load for the whole child pair (inconsistent)

The only variant that actually **removes** an instruction (objdump-confirmed:
**19 per literal bit vs stock's 20**). With 32-bit probs the two children are a
naturally-aligned 8-byte pair at `8*sym`, so `ldr x8,[probs,sym,lsl#3]` fetches
both; the low child is the free `w8` view and the high child is one `lsr`.
Keeping `sym` undoubled then also collapses `add sym,sym` + `adc sym,sym,wzr`
into a single `adc sym,sym,sym`, and makes the parent store a plain
`str [probs,sym,lsl#2]` (under `Z7_LZMA_PROB32` the stock `PSTORE_LSL_M1` needs
an extra `add`). Alignment of every sub-table base was checked by hand and all
are 8-byte aligned.

Measured **four times, and it does not have a consistent sign**:

| run | stock | 03-pair32 | delta |
|---|---|---|---|
| 32 MiB slice, quiet | 0.4767–0.4807 | 0.4812 | +0.5% |
| 256 MiB fixture, loaded | 3.7806 | 3.7591 (PROB32 only) | −0.6% |
| 256 MiB fixture, quiet | 3.8025 | 3.9784 | **+4.5%** |
| 1024 MiB fixture, quiet | 16.1150 | 15.8416 | **−1.7%** |

A change that is 4.5% slower on one fixture and 1.7% faster on another is not a
win; it is a change whose effect is smaller than the run-to-run and
fixture-to-fixture variation. Two plausible mechanisms, neither separable
without counters: (a) the loop is chain-bound at that point so −1 instruction
is free but worthless, and (b) the 4-byte prob store partially overlaps the
later 8-byte pair load of the same pair — classic partial store-to-load
forwarding. Mechanism (b) is supported by `Z7_LZMA_PROB32` *without* the pair
load measuring neutral (3.8063 vs stock 3.8025 on the 256 MiB fixture), which
exonerates the doubled prob table and points at the 8-byte load. Writing the
store as a full 8-byte pair store to dodge (b) needs 4 extra instructions to
splice the updated half in, far more than it could save.

**Not adopted**: zero measured gain in exchange for an ABI-visible change
(`CLzmaProb` must be 32-bit in `LzmaDec.h`, `LzmaDec.c` and the asm
simultaneously) and 2x prob-table memory.

### `04-idx32` — never assembled

Same instruction saving as 03 without the 64-bit load, using two `ldr w` from
`probs` and `probs+PMULT` indexed by the undoubled symbol. Requires `lsl #3` on
a 32-bit load: `error: expected 'lsl' or 'sxtx' with optional shift of #0 or
#2`. Deleted.

### `05-normless` — branchless renormalisation (+9.1%, worst result)

Replaces `tst range,0xFF000000 / b.ne` + the branched-over 3-instruction refill
with 7 unconditional instructions (`lsl`/`csel` for range, speculative
`ldrb [buf]`, `orr`/`csel` for code, `cinc buf`), using only `t0` as scratch —
every other temp is live in at least one `NORM` call site (`t1` is
`probs_state`, `t5` is LITM's `match`, `w2` is LITM's `prm`).

This was my best *a priori* idea and it is the clearest loss of the session.
The conclusion is that **the renormalisation branch predicts well** — well
enough that removing ~5 net instructions' worth of mispredict exposure is a bad
trade against paying those 5 instructions unconditionally. Renorm fires roughly
1 bit in 8 and is evidently far from random to the predictor.

(It would also have added an invariant: a speculative `ldrb [buf]` on every bit
reads up to one byte past `bufLimit`.)

### Diagnostics (not candidates)

* `d1-sym-plus1` / `d2-range-plus1` / `d3-cod-plus1` — insert `eor X,X,wzr` into
  the sym / range / cod chains to find which recurrence binds.
* `d4-plus1insn` / `d5-plus3insn` — the +1/+3 independent instruction probes
  that produced the 0.32 cyc/instruction figure.
  *Both initially SIGSEGV'd* because the filler used `w26`/`w16`/`w27`, which
  are live (`probs_Spec`, `bufLimit`, `checkDicSize`); fixed by using `w8`/`w4`/
  `w6`, which the same macro overwrites anyway.

---

## 5. Checklist items that needed no change

* **`.p2align 5` on loop heads.** Stock already defines
  `MY_ALIGN_FOR_ENTRY = MY_ALIGN_FOR_LOOP = MY_ALIGN_32`, and `lit_start` is
  `MY_ALIGN_64`. Confirmed in `objdump -d`. Nothing to do.
* **Literal decode unrolling.** `_LZMA_SIZE_OPT` is off by default, so the
  literal tree is already fully unrolled (8 `BIT_*` copies, confirmed by the
  8 evenly-spaced `mul` instructions in the disassembly). The `#ifdef
  _LZMA_SIZE_OPT` rolled loop is the *slow* path and is not built.
* **Matched-literal (LITM) masked vs branch.** Already fully masked/branchless
  in stock (`and bit,match,offs` + `cmovae offs,bit`), and it is nearly cold on
  these fixtures.
* **Register residency / no spills.** Stock spills nothing in the hot loop; it
  is x0–x17 + x19–x30 with x18 reserved. Verified by reading the prologue
  (`stp` of x19–x30 only) and by the absence of `sp`-relative traffic in the
  literal blocks.

## 6. Workload characterisation

Both project fixtures are literal-dominated (compression ratios 0.92 and 0.875),
so ~97% of profile samples land in the plain-literal path; matched-literal and
match-copy are nearly cold. The same decoder runs at **~66 MiB/s** on these and
**~182 MiB/s** on a genuinely compressible stream, which is worth remembering:
the fixtures measure the literal bit-tree almost exclusively, and any future
work on the match/copy paths will be invisible on them.

## 7. Ideas explicitly considered and rejected without building

* **32-bit-at-a-time renormalisation.** Would shorten the refill a lot, but it
  changes where `buf` ends up, which breaks drop-in compatibility with
  `LzmaDec_TryDummy` and `LzmaDec.c`'s buffer accounting. Interesting for a
  from-scratch Rust decoder that owns both sides; not for a drop-in `.S`.
* **Two tree levels per addressing op** (load 4 grandchildren). `ldp` has no
  register-indexed addressing form.
* **Fusing the renorm test into one instruction.** `range < 2^24` is not a
  single-bit predicate, so `tbz`/`cbz` cannot express it; maintaining a biased
  `range - 2^24` to make it sign-testable costs a correction on every use.
* **A 2-instruction probability update.** Pavlov's `kBitModelOffset = 2017`
  already fuses the two update rules into `sub` + `csel` + `sub ..., asr #5`.
  The identity `(p-2017)>>5 == (p>>5)-63` fails exactly when `p % 32 == 0`, so
  the cheaper-looking forms are wrong, and every correct alternative I derived
  also needs three operations.

---

## 8. Final numbers vs the shipped `7zz`

`7zz` 26.01 (arm64, ASM) from Homebrew. Min-of-N child CPU seconds, neither
side writing output (`7lzma d <in> /dev/null` vs `7zz t -mmt=1`). Load average
~7 during these runs.

| fixture | decoder | s (cpu) | MiB/s | ratio vs 7zz |
|---|---|---|---|---|
| `p256.bin.lzma` (256 MiB, ratio 0.92) | `7zz t -mmt=1` | 3.8343 | 66.8 | 1.000 |
| | **m5 asm loop** | **3.8630** | **66.3** | **1.007** |
| | 7-Zip C loop | 5.6797 | 45.1 | 1.481 |
| `payload.bin.lzma` (1024 MiB, ratio 0.875) | `7zz t -mmt=1` | 14.8651 | 68.9 | 1.000 |
| | **m5 asm loop** | **14.9701** | **68.4** | **1.007** |

`7zz` runs the *same* decode asm, so parity is the expected outcome and the
residual is CLI/IO overhead rather than decode speed. **The "ratio below 1.0"
target was not met**; the reason is the whole of sections 2–4 above.

The row that matters for the Rust port is the C loop: the asm is **1.47x
faster** than 7-Zip's own optimised C decoder, and that is the bar to clear.
