# Releasing

## What is published

One version covers the firmware, every published crate and the Python
package: `[workspace.package]`'s in the root `Cargo.toml`, and
`firmware/pod-fw/Cargo.toml`'s.  A `v*` tag publishes, from
`.github/workflows/release.yml`.

| Package | Where | Built by |
| --- | --- | --- |
| `mc683xx-bdm` (Debian, `bdm`, the udev rule and the UF2), amd64 and arm64 | GitHub Release | deb-workflows' `rust-build-deb` |
| `pod-fw-rp2040.uf2` | GitHub Release | `make uf2` (elf2uf2-rs) |
| `mc683xx-bdm-proto`, `mc683xx-bdm` (the library and `bdm`) | crates.io | `cargo publish --workspace`: the first release by hand, then the `crates` job |
| `mc683xx-bdm` (the Python package, import `mc683xx_bdm`, from `crates/py`) | PyPI | the `wheels`, `sdist` and `pypi` jobs |

Every other member has `publish = false`: `mc683xx-bdm-model` (a test
target), `mc683xx-bdm-stand-in` (`bdm-stand-in`, over the model) and
`mc683xx-bdm-py` (its wheel is its release).  `mc683xx-bdm-proto` and
`mc683xx-bdm` depend on `mc683xx-target` and `mc683xx-gdb` from
crates.io (mc683xx's), and `mc683xx-bdm` on `mc683xx-bdm-proto` at
exactly its own version (`[workspace.dependencies]`).

The published crates carry no tests that build.  A published crate's
`include` keeps its package to `src/`, its README and the license text
(and `mc683xx-bdm`'s build script and the `git-version.rs` it
includes).  `mc683xx-bdm`'s tests need `mc683xx-bdm-model`, a
path-only dev-dependency that `cargo publish` drops; they run from the
repository.  `bdm version` from a package names the commit cargo
recorded in it (`.cargo_vcs_info.json`).

`crates/proto`, `crates/host` and `crates/py` hold copies of the root's
`LICENSE`, so the packages, the wheels and the sdist carry it
(`license-files` in `crates/py/pyproject.toml`).  `make lint` checks
that they match the root's byte for byte; a change to the license text
is made at the root and copied.

The wheels are abi3 (Python 3.10 and later), one per platform: Linux
manylinux x86_64 and aarch64, macOS x86_64 and arm64, Windows x64, and
an sdist.  Linux x86_64's and macOS arm64's run the Python tests on
Python 3.10 first, against `bdm-stand-in` on a Unix socket; the
cross-built ones and Windows', which has no Unix sockets (`--pod
unix:PATH` is Unix's), are only built.  The `sdist` job builds a wheel
from the sdist, as pip does where no wheel fits, and checks it carries
the stubs and `py.typed`.

A pull request or a push to main runs every build (`firmware`, `deb`,
`package`, `wheels`, `sdist`) and publishes nothing; `package` is
`cargo publish --workspace --dry-run`, which builds `mc683xx-bdm`
against the packaged `mc683xx-bdm-proto` before either is on
crates.io.

## Setup

Once, before the first release, after the repository is public.

1. PyPI: add a pending trusted publisher for the project `mc683xx-bdm`
   (pypi.org, Your projects, Publishing, Add a new pending publisher,
   GitHub): owner `charlieh0tel`, repository `mc683xx-bdm`, workflow
   `release.yml`, environment `pypi`.  Until it exists, the `pypi` job
   fails and nothing is published there.
2. GitHub (Settings, Environments): create the environments `pypi` and
   `crates-io`, with no required reviewers.  For each, under deployment
   branches and tags choose selected branches and tags and add a tag
   rule `v*`, so only a `v` tag's run can deploy to it.
3. crates.io: trusted publishing can only be set up for a crate that
   already exists, so the first release of each crate is published by
   hand (Cutting a release, step 9), with an API token (scope
   `publish-new`, crates `mc683xx-bdm*`) in `cargo login`, revoked
   after.  Then, for each of `mc683xx-bdm-proto` and `mc683xx-bdm`
   (crate settings, Trusted Publishing, Add): owner `charlieh0tel`,
   repository `mc683xx-bdm`, workflow `release.yml`, environment
   `crates-io`; a field that differs fails that crate's publish.  Every
   later release is published by the workflow only.

No repository secret is needed: both registries mint a short-lived
token for the run (`id-token: write` on the publishing jobs only).

## Cutting a release

Firmware and host are released together: the pod and `bdm` refuse each
other unless their `PROTOCOL_VERSION` (`crates/proto/src/usb.rs`)
matches.

1. Pick the version: before 1.0, a breaking change to a public API
   (Rust, Python or the protocol) bumps the minor version, anything
   else the patch.
2. On a branch, bump the version in the root `Cargo.toml`
   (`[workspace.package]`'s `version` and each published crate's
   `=X.Y.Z` in `[workspace.dependencies]`) and in
   `firmware/pod-fw/Cargo.toml` (its `version` and its
   `mc683xx-bdm-proto` dependency's); `cargo check --workspace` and
   `cd firmware/pod-fw && cargo check` update the lockfiles.  The
   wheel's version follows the workspace's.  Add a
   `packaging/debian/changelog` entry.  Raise `PROTOCOL_VERSION` and
   `docs/usb-protocol.md` if the wire changed.
3. `make lint`, `cargo test --locked`, `cargo publish --workspace
   --dry-run --locked`, and in `crates/py` `uv build`.
4. `make deb`: the UF2 (`firmware/pod-fw/target/pod-fw-rp2040.uf2`;
   picotool 2, else elf2uf2-rs) and the package, which ships it.
5. Report flash, RAM and stack peak (AGENTS.md, Stack and RAM).  `bdm
   version` names the commit for both; neither may say `-dirty`.
6. On a target: `docs/bring-up.md` steps 5-12.
7. Commit as "Release X.Y.Z", merge it to `main`, and wait for `CI`
   to pass on the merged commit.
8. Tag that commit `vX.Y.Z`, annotated, and push the tag.  `release`
   builds the UF2, then `rust-build-deb` builds, tests and audits
   `mc683xx-bdm` for amd64 and arm64 and publishes a GitHub Release of
   both packages; the UF2 is attached to it.  Its `gate` job checks
   that the tagged commit is on `main`, that the tag names both
   versions, and, through the GitHub API, that `CI`'s latest run of
   that commit's push to `main` succeeded; a tag whose `gate` failed
   for a `CI` run still going is re-run (Actions, Re-run jobs) once
   `CI` has passed.  The `pypi` job uploads the wheels and sdist only
   once `gate` and the builds have passed.  A release that failed
   partway is finished by re-running its failed jobs: each publishing
   job skips what its registry already has.
9. The `crates` job, after `gate`, `package` and `pypi`, runs `cargo
   publish --workspace` with a trusted-publishing token.  A release
   that adds a crate, the first among them: that job fails, as a
   trusted-publishing token cannot create a crate, and publishes
   nothing, cargo checking every package first.  Publish by hand from a
   clean checkout of the tag.  A crate already published takes new
   versions by trusted publishing only, so first turn off "require
   trusted publishing" in its crates.io settings, and turn it back on
   after.  Start from no unpacked copy of an earlier attempt:

   ```
   export CARGO_HOME="$(mktemp -d)"
   cargo login            # publish-new, and publish-update if a crate exists
   cargo publish --workspace --locked
   ```

   Cargo uploads `mc683xx-bdm-proto`, waits for it to reach the index,
   then `mc683xx-bdm`.  If it stops partway, go on with `--exclude`
   for a crate already up (`scripts/published-crates.sh` lists them).
   Then revoke the token, and add the trusted publishers (Setup,
   step 3).

A broken release is yanked (`cargo yank`, PyPI's yank) and fixed in the
next patch version; a version is never published twice.

## Carrier board

1. Bump REV in both title blocks.
2. `jlcpcb-release` skill: package in `hardware/production/YYYYMMDD-REV/`,
   then its ordering checklist.
3. Tag `mc683xx-pod-YYYYMMDD-REV` on the commit ordered.  The production
   directory is not changed afterwards.
