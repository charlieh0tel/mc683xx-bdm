# Releasing

## Firmware and `bdm`

Released together: the pod and `bdm` refuse each other unless their
`PROTOCOL_VERSION` (`crates/proto/src/usb.rs`) matches.

1. Bump `version` in `crates/*/Cargo.toml` and
   `firmware/pod-fw/Cargo.toml`; add a
   `packaging/debian/changelog` entry.  Raise `PROTOCOL_VERSION` and
   `docs/usb-protocol.md` if the wire changed.
2. `make lint`, `cargo test --locked`.
3. `make deb`: the UF2 (`firmware/pod-fw/target/pod-fw-rp2040.uf2`;
   picotool 2, else elf2uf2-rs) and the package, which ships it.
4. Report flash, RAM and stack peak (AGENTS.md, Stack and RAM).  `bdm
   version` names the commit for both; neither may say `-dirty`.
5. On a target: `docs/bring-up.md` steps 5-12.
6. Tag `vX.Y.Z` and push the tag.  `.github/workflows/release.yml`
   builds the UF2 (elf2uf2-rs), then deb-workflows' `rust-build-deb`
   builds, tests and audits `mc683xx-bdm` for amd64 and arm64 and
   publishes a GitHub Release of both packages; the UF2 is attached to
   it.  A pull request or a push to main runs the same build and
   publishes nothing.

## Carrier board

1. Bump REV in both title blocks.
2. `jlcpcb-release` skill: package in `hardware/production/YYYYMMDD-REV/`,
   then its ordering checklist.
3. Tag `mc683xx-pod-YYYYMMDD-REV` on the commit ordered.  The production
   directory is not changed afterwards.
