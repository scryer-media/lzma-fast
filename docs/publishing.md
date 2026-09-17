# Publishing

Releases are cut with `scripts/release.sh` and published by
`.github/workflows/release.yml`. The script runs every check the workflow
runs, so a tag that reaches GitHub is one the workflow will accept.

## One-time setup, before the first release

1. **Publish the first version by hand.** crates.io only lets a workflow
   publish a crate that already exists, so the first version goes up from a
   machine with a crates.io token:

   ```sh
   cargo publish -p lzma-fast --locked
   ```

   Do this after the tag is pushed and the `verify` job is green; the
   `publish` job of that first run will fail, which is expected.
2. **Turn on trusted publishing** at
   `https://crates.io/crates/lzma-fast/settings/new-trusted-publisher`:
   repository owner `scryer-media`, repository `lzma-fast`, workflow
   `release.yml`, environment `crates-io`.
3. **Create the `crates-io` environment** in the GitHub repository settings.
   Restricting it to tag refs `v*` is enough; no secrets are needed, the
   workflow gets a short-lived token from crates.io through OIDC.

The release workflow needs `contents: write` to create the GitHub release,
which it requests for that job only.

## Each release

1. On `main`, with CI green on the merge commit:
   - set `version` in `Cargo.toml` (`[workspace.package]`) and the `lzma-fast`
     entry under `[workspace.dependencies]`;
   - change the version's `crates/lzma-fast/CHANGELOG.md` heading from
     `(unreleased)` to the date, `## 0.3.0 - 2026-09-17`;
   - update the two dependency lines in `crates/lzma-fast/README.md`;
   - commit, signed.
2. `scripts/release.sh --dry-run`, then `scripts/release.sh`. The script
   refuses a dirty tree, an unsigned HEAD, a branch other than `main`, a
   changelog section still marked unreleased, a README that does not show the
   version, and a tag that already exists. It then runs the tests and a
   `cargo publish --dry-run`, creates the signed tag `v<version>` and pushes
   it.
3. The workflow verifies the tag against the manifest, tests, publishes to
   crates.io and creates the GitHub release with the changelog section as its
   notes. Nothing else needs doing; `sevenz-fast` is released separately once
   this version is on crates.io.
