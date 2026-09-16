# Publishing

1. On `main`, ensure `cargo test --workspace`, clippy and `cargo doc` are green
   and CI passed on the merge commit.
2. Bump `version` in `Cargo.toml` (`[workspace.package]`) and the
   `lzma-fast` entry under `[workspace.dependencies]`; update
   `crates/lzma-fast/CHANGELOG.md`; commit (signed).
3. `cargo package -p lzma-fast --list` and check no fixture bytes are included.
4. `cargo publish -p lzma-fast`.
5. Tag `v<version>` (signed) and push the tag.
