# Agent and contributor rules for lzma-turbo

These rules apply to every automated agent and every human contributor.

## What this repository is

A decode-only LZMA/LZMA2 library, ported from Igor Pavlov's public-domain
`LzmaDec.c` / `Lzma2Dec.c`. The point of the crate is speed with parity to
the reference decoder. Readability that costs throughput is not wanted.

## Porting rules

1. Port, do not redesign. Keep the reference decoder's structure: one decode
   loop, coder state in locals, per-symbol limit checks under a margin, a
   separate slow path near buffer edges. See `docs/porting.md`.
2. The reference is `C/` in a checkout of github.com/ip7z/7zip at the commit
   named in `docs/porting.md`. Cite the C function name in a comment when a Rust
   function corresponds to one.
3. `unsafe` is allowed where the C relies on the margin invariant, and only
   there. Every `unsafe` block carries a `// SAFETY:` comment stating the
   invariant that makes it sound and which check established it.
4. No dependencies in the decoder modules: `src/lzma/`, `src/lzma2/` and
   everything they reach must build from `core` and `alloc` alone. The only
   library dependencies the crate may take are `crc-fast`, behind the `crc`
   feature, and the SHA-256 backends — `aws-lc-rs` behind `crypto`, `sha2`
   behind `native-crypto` — all optional, all confined to `src/crc.rs` and
   `src/crypto/`, and none of them reachable from the decoder even when the
   default features have them on. Anything else needs a decision
   from the maintainer, not from an agent. Dev-dependencies and the bench tool
   may pull what they need.
5. Correctness is proven differentially: decoded bytes must equal what
   `xz -dc` / `7zz` produce for the same input, byte for byte, and malformed
   input must return an error rather than panic or read out of bounds.

## Performance rules

- The acceptance gate is `docs/benchmarking.md`. A change that regresses the
  gate does not merge, however clean it is.
- Measure with `cargo run --release -p lzma-bench`, on an otherwise idle
  machine, at least three runs, report the median.
- Never commit fixture bytes. `cargo xtask fixtures` regenerates them.

## Repository hygiene

- Commits are SSH-signed. Never pass `--no-gpg-sign`.
- Never run destructive git working-tree operations (`checkout --`,
  `restore`, `reset --hard`, `clean`, `stash`) in a shared checkout.
- Branch names use gitflow prefixes: `feature/…`, `bugfix/…`, `hotfix/…`.
- Do not push. The maintainer pushes.
- The pre-commit hook (`.githooks/pre-commit`) rejects home paths, the local
  username and secrets. Keep it enabled: `git config core.hooksPath .githooks`.
- `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`
  must pass before a commit is proposed for review.
- Any code change to the crate (`src/`) bumps its version, `Cargo.lock` and
  `CHANGELOG.md` in the same change.
