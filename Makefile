# afa workspace — single source of truth for the validation suite.
#
# After every implementation, type `make` to run the four commands
# below in order. `fmt` first (cheap, deterministic), then `clippy`
# (lints), then `test` (behaviour), then `doc` (docs still build).
# If any step fails, `make` stops at the first one.

.PHONY: check
check:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	cargo doc --workspace --no-deps
