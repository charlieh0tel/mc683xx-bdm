# Repository chores.  The firmware and host build with cargo in
# firmware/, the hardware release with hardware/release.py.

.PHONY: hooks lint

# Use the repository's git hooks (.githooks/), once per clone.
hooks:
	git config core.hooksPath .githooks

# Before each commit (AGENTS.md): clippy on the workspace and on each
# firmware build, warnings as errors, and the hardware tooling's checks.
# Firmware builds, one per chip (PLAN.md, Firmware).
CHIPS = rp2040 rp235xa
lint:
	@# No wildcard imports, `use super::*` in tests included (AGENTS.md):
	@# clippy's wildcard_imports does not report these.
	@if git grep -nE '^\s*(pub(\([a-z]+\))? )?use [^;]*::\*;' -- 'firmware/*.rs'; then \
		echo "wildcard imports (above)"; exit 1; fi
	@if [ -f firmware/Cargo.toml ]; then \
		cd firmware && cargo clippy --all-targets -- -D warnings; \
	else echo "lint: no firmware/Cargo.toml yet; workspace clippy skipped" >&2; fi
	@if [ -f firmware/pod-fw/Cargo.toml ]; then \
		for chip in $(CHIPS); do \
			(cd firmware/pod-fw && cargo clippy --release --no-default-features \
				-F $$chip --bins -- -D warnings) || exit 1; \
		done; \
	else echo "lint: no firmware/pod-fw yet; firmware clippy skipped" >&2; fi
	cd hardware && uv run ruff format --check . && uv run ruff check . && uv run mypy .
