# Contributing

Thanks for looking. A few things before you open a pull request:

- Read [AGENTS.md](../AGENTS.md); the porting and performance rules there
  apply to humans too.
- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` locally.
- If you touched the decoder, include before/after numbers from
  `cargo run --release -p lzma-bench` per `docs/benchmarking.md`.
- Bump the crate version, `Cargo.lock` and the crate changelog in the
  same PR as the code change.
- Enable the hooks once per clone: `git config core.hooksPath .githooks`.
