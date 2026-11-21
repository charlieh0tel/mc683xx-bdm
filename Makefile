# Repository chores.  The host crates build with cargo at the root, the
# firmware in firmware/pod-fw; the hardware release is ee_tooling's
# jlcpcb-release.

.PHONY: deb hooks lint size uf2

# cargo runs here with --locked and uv with UV_LOCKED: a Cargo.lock or
# uv.lock that does not match its manifests is an error, not quietly
# re-resolved.
export UV_LOCKED := 1

# The pod's firmware as a UF2 (RP2040, the Pico), which the package ships.
UF2 := firmware/pod-fw/target/pod-fw-rp2040.uf2

# The bdm Debian package, as rust-build-deb (deb-workflows) builds it,
# with the UF2: target/debian/mc683xx-bdm_*.deb.
deb: uf2
	cargo deb --locked -p mc683xx-bdm

# picotool 2's `uf2 convert` where it is installed, else elf2uf2-rs
# (the release workflow's): both write the RP2040's family ID.
POD_FW_ELF := firmware/pod-fw/target/thumbv6m-none-eabi/release/pod-fw

uf2:
	cd firmware/pod-fw && cargo build --locked --release --no-default-features -F rp2040
	if command -v picotool > /dev/null; then \
		picotool uf2 convert -t elf $(POD_FW_ELF) $(UF2); \
	else \
		elf2uf2-rs $(POD_FW_ELF) $(UF2); \
	fi

# Use the repository's git hooks (.githooks/), once per clone.
hooks:
	git config core.hooksPath .githooks

# Firmware builds, chip:target, each with and without rtt (PLAN.md,
# Firmware).
BUILDS = rp2040:thumbv6m-none-eabi rp235xa:thumbv8m.main-none-eabihf

# The rustup toolchain's llvm-size (its llvm-tools component).
LLVM_SIZE = $(shell rustc --print sysroot)/lib/rustlib/$(shell rustc -vV | sed -n 's/^host: //p')/bin/llvm-size

# Every firmware build linked, release, and its flash and RAM (AGENTS.md,
# Stack and RAM): llvm-size's text is flash, data flash and RAM, bss
# RAM, against the memory map's FLASH and RAM (memory-*.x), which the
# link refuses to overflow.
size:
	@for build in $(BUILDS); do \
		chip=$${build%%:*}; target=$${build#*:}; \
		for extra in "" ,rtt; do \
			(cd firmware/pod-fw && cargo build --locked --release --no-default-features \
				-F $$chip$$extra --target $$target) || exit 1; \
			echo "$$chip$$extra:"; \
			$(LLVM_SIZE) firmware/pod-fw/target/$$target/release/pod-fw || exit 1; \
		done; \
	done

# Before each commit (AGENTS.md): rustfmt, clippy on the workspace (the
# Python bindings with their `python` feature too) and on every firmware
# build, warnings as errors, ee_tooling's schematic drawing-rule check,
# ruff and pytest on hardware/'s own Python, and ruff, pytest, mypy's
# stubtest of the type stubs against the module and mypy on the tests
# for the Python bindings (crates/py), which uv builds with
# maturin.
lint:
	@# No wildcard imports, `use super::*` in tests included (AGENTS.md):
	@# clippy's wildcard_imports does not report these.
	@if git grep -nE '^\s*(pub(\([a-z]+\))? )?use [^;]*::\*;' -- 'crates/*.rs' 'firmware/*.rs'; then \
		echo "wildcard imports (above)"; exit 1; fi
	cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings
	cargo clippy --locked -p mc683xx-bdm-py --all-targets --features python -- -D warnings
	cd firmware/pod-fw && cargo fmt --check
	for build in $(BUILDS); do \
		chip=$${build%%:*}; target=$${build#*:}; \
		for extra in "" ,rtt; do \
			(cd firmware/pod-fw && cargo clippy --locked --release --no-default-features \
				-F $$chip$$extra --target $$target --bins -- -D warnings) || exit 1; \
		done; \
	done
	cd hardware && uv run kicad-style-check mc683xx-pod/mc683xx-pod.kicad_sch
	cd hardware && uv run ruff format --check && uv run ruff check && uv run pytest -q
	cd crates/py && uv run ruff format --check && uv run ruff check && uv run pytest -q \
		&& uv run python -m mypy.stubtest mc683xx_bdm --allowlist stubtest-allowlist.txt \
		&& uv run mypy tests
