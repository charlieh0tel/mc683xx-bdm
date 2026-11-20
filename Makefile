# Repository chores.  The firmware and host build with cargo in
# firmware/; the hardware release is ee_tooling's jlcpcb-release.

.PHONY: deb hooks lint

# The bdm Debian package, as rust-build-deb (deb-workflows) builds it:
# firmware/target/debian/mc683xx-pod_*.deb.
deb:
	cd firmware && cargo deb -p pod-host

# Use the repository's git hooks (.githooks/), once per clone.
hooks:
	git config core.hooksPath .githooks

# Firmware builds, chip:target, each with and without rtt (PLAN.md,
# Firmware).
BUILDS = rp2040:thumbv6m-none-eabi rp235xa:thumbv8m.main-none-eabihf

# Before each commit (AGENTS.md): rustfmt, clippy on the workspace and on
# every firmware build, warnings as errors, ee_tooling's schematic
# drawing-rule check, and ruff and pytest on hardware/'s own Python.
lint:
	@# No wildcard imports, `use super::*` in tests included (AGENTS.md):
	@# clippy's wildcard_imports does not report these.
	@if git grep -nE '^\s*(pub(\([a-z]+\))? )?use [^;]*::\*;' -- 'firmware/*.rs'; then \
		echo "wildcard imports (above)"; exit 1; fi
	cd firmware && cargo fmt --check && cargo clippy --all-targets -- -D warnings
	cd firmware/pod-fw && cargo fmt --check
	for build in $(BUILDS); do \
		chip=$${build%%:*}; target=$${build#*:}; \
		for extra in "" ,rtt; do \
			(cd firmware/pod-fw && cargo clippy --release --no-default-features \
				-F $$chip$$extra --target $$target --bins -- -D warnings) || exit 1; \
		done; \
	done
	cd hardware && uv run kicad-style-check mc683xx-pod/mc683xx-pod.kicad_sch
	cd hardware && uv run ruff format --check && uv run ruff check && uv run pytest -q
