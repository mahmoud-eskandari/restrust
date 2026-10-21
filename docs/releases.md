# Building and releasing

The **CI** workflow runs on pull requests, pushes to `main` or `codex/**`, and manual dispatch. It validates workflows with actionlint, runs formatting and strict Clippy checks, and tests the workspace before building release binaries. Every native build runs on its matching architecture; cross-compilation is not used.

| Target | Hosted runner | Release archive |
| --- | --- | --- |
| Linux x64 | Ubuntu 24.04 | `restrust-VERSION-x86_64-unknown-linux-gnu.tar.gz` |
| Linux ARM64 | Ubuntu 24.04 ARM | `restrust-VERSION-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Intel | macOS 15 Intel | `restrust-VERSION-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | macOS 15 ARM | `restrust-VERSION-aarch64-apple-darwin.tar.gz` |
| Windows x64 | Windows Server 2025 | `restrust-VERSION-x86_64-pc-windows-msvc.zip` |
| Browser | Ubuntu 24.04 / WASM | `restrust-VERSION-web.zip` |

Native archives contain both `restrust-client` and `restrust-server` (`.exe` on Windows), documentation, and licenses. The packaging step starts the built server with an isolated temporary database and checks its version and HTTP health endpoint. The desktop executable is built and its shared logic is tested; CI does not interact with a desktop window. Executables are unsigned. Linux packages target Ubuntu 24.04-compatible systems and require the usual graphics libraries for the desktop app; they are not static musl binaries.

The browser archive contains a `web/` directory built by Trunk. Set `RESTRUST_WEB_DIR` to its absolute path when starting the server. Browser sends require signing in to this server, as described in [Getting started](getting-started.md).

## Download a CI build

Open a completed CI run in GitHub Actions and download the appropriate `release-TARGET` artifact. It contains a release archive and a matching `.sha256` file. On Linux or macOS, extract the archive to preserve executable permissions. On Windows, extract the ZIP and launch the `.exe`.

## Create a versioned release

1. Update `workspace.package.version` in `Cargo.toml` and refresh `Cargo.lock` with Cargo.
2. Commit the release version, then push a matching tag such as `v0.1.0`. A stable workspace version also accepts a prerelease suffix: with `0.1.0` in Cargo, tags such as `v0.1.0-beta`, `v0.1.0-beta.1`, and `v0.1.0-rc.1` are valid. A different base version, such as `v0.2.0-beta`, is rejected. If Cargo already contains a prerelease version, the tag must match it exactly.
3. The **Release** workflow validates the tag and applies its complete version to the temporary CI checkouts. It refreshes workspace lockfile entries with `cargo update --workspace`, then runs the complete CI workflow with locked dependencies. Binaries and archives therefore carry the full version, including `-beta`. It downloads all six archives and verifies their checksums.
4. If no release exists, it creates a **draft** GitHub release with the archives and `SHA256SUMS`. Review the draft and publish it from GitHub when ready. If you already published a release or prerelease for the tag, the workflow adds the missing files to that existing release.

Prerelease tags also set GitHub's **Pre-release** flag. Stable tags leave that flag off. Version preparation only changes the CI checkout; it does not commit or push changes to the repository.

Create a tag on the commit containing the current release workflow. Rerunning an old failed tag run uses its original commit and workflow; merging a fix into `main` does not update that run.

Rerunning the tag workflow can replace assets in an existing draft. On a published release, existing files must match their SHA-256 checksums; matching files are skipped and different files cause an explicit error before any uploads. Published release metadata and existing files are preserved. Manual CI runs only produce Actions artifacts and do not create a release.

## Retry only the asset upload

If tests and builds passed but uploading failed, use the **Upload release assets** workflow after the fix is merged into the default branch:

1. Open **Actions → Upload release assets → Run workflow** and select `main`.
2. Enter the existing tag, for example `v0.1.0-beta`.
3. Enter the original **Release** run ID from its URL (`actions/runs/RUN_ID`).

This workflow verifies that the run belongs to the same repository and tag commit and that every version, test, and build job succeeded. It downloads the existing six artifacts, checks every archive checksum, and uploads missing files. It does not rebuild binaries, recreate tags, or change a published release back to a draft. The source artifacts must still be available; GitHub normally retains them for 90 days unless repository settings specify otherwise. An immutable release cannot receive missing assets.

## Local checks

```sh
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
actionlint
python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v
```

To build and package for the current native host, substitute its target triple from the table:

```sh
cargo build --locked --release --workspace --bins --target x86_64-unknown-linux-gnu
python3 scripts/ci/package_release.py --target x86_64-unknown-linux-gnu
```

For the browser, run `trunk build --release --locked` in `client/`, then `python3 scripts/ci/package_release.py --web` from the repository root. Packaging scripts require Python 3.11 or newer. Output goes to the ignored `dist/` directory.
