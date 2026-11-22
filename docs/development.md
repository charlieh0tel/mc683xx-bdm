# Development

Once per clone: `git submodule update --init` (ee_tooling, which the
commit hook runs from), then `make hooks`. For the hardware tools,
`cd hardware && uv venv --system-site-packages --python /usr/bin/python3
&& uv sync`. [AGENTS.md](../AGENTS.md) has the rules,
[PLAN.md](../PLAN.md) the design, [RELEASING.md](../RELEASING.md) the
release steps.

## Layout

| Path | What |
|---|---|
| `crates/proto/` | `mc683xx-bdm-proto`: the protocol and request rules, shared by the firmware and the host |
| `crates/host/` | `mc683xx-bdm`: the host library and the `bdm` CLI |
| `crates/py/` | `mc683xx-bdm-py`: the Python module `mc683xx_bdm` |
| `crates/model/` | `mc683xx-bdm-model`: a model CPU32, for tests |
| `crates/stand-in/` | `mc683xx-bdm-stand-in`: `bdm-stand-in`, a pod on a socket over the model |
| `firmware/pod-fw/` | the pod's firmware, its own workspace |
| `hardware/` | the KiCad carrier and its revisions (`README.md`); `production/`, each release as ordered; `ee_tooling/`, the shared tooling (a submodule); `fastcheck.py`, a report on the BDM link's fast nets |
| `docs/` | the host-pod protocol (`usb-protocol.md`), bring-up steps (`bring-up.md`), the pod on an HP 58503A (`58503a/`); the Oncore VP's notes are smartclock's ([oncore-vp-board.md](https://github.com/charlieh0tel/smartclock/blob/main/docs/oncore-vp-board.md), [test points](https://charlieh0tel.github.io/smartclock/oncore-vp-test-points.html)), images (`make render`) |
| `packaging/` | the Debian changelog and the udev rule |
| `scripts/` | `published-crates.sh`, for the release workflow |
| `third_party/` | vendor documents and their provenance (`NOTICE`) |

## Build and test

```
cargo test                     # every crate in crates/, on the host
make lint                      # clippy on every build, before each commit
make size                      # every firmware build linked; flash and RAM

cd firmware/pod-fw
cargo run --release            # Pico, defmt over RTT, through an SWD probe
cargo build --release --no-default-features -F rp2040   # production
cargo build --release --no-default-features -F rp235xa \
    --target thumbv8m.main-none-eabihf                   # Pico 2
```

## A pod without hardware

`bdm-stand-in` serves the pod's protocol on a socket over the model
CPU32:

```
cargo run -p mc683xx-bdm-stand-in -- unix:/tmp/pod.sock &
cargo run -p mc683xx-bdm --bin bdm -- --pod unix:/tmp/pod.sock reset
```

## Python module

```
cd crates/py
uv sync                        # builds and installs the module (maturin)
uv run pytest                  # against bdm-stand-in, no hardware
uv build --wheel
```

`make lint` checks `python/mc683xx_bdm/__init__.pyi` against the module
with mypy's stubtest.

## Debian package

`make deb` builds `target/debian/mc683xx-bdm_VERSION-1_ARCH.deb`: `bdm`,
the udev rule and the firmware (`/usr/share/mc683xx-bdm/pod-fw-rp2040.uf2`,
built with picotool 2, else elf2uf2-rs), with cargo-deb 3.8.0
(deb-workflows' `rust-build-deb` pin). Metadata:
`[package.metadata.deb]` in `crates/host/Cargo.toml`; changelog:
`packaging/debian/changelog`. A `v*` tag releases it through
`.github/workflows/release.yml`.
