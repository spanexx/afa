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

# teach-serve — start the local companion for the .agents/skills/teach/
# skill. Serves teach/<topic>/lessons/*.html on http://127.0.0.1:8765
# and accepts POST /api/questions so the user can leave a question on
# a lesson page and have the agent pick it up next session.
#
# Foreground by design (Ctrl-C to stop). Override port via TEACH_PORT.
# Run `make -C afa teach-selftest` once to verify the server end-to-end.
.PHONY: teach-serve teach-selftest
teach-serve:
	@cd .. && python3 .agents/skills/teach/scripts/teach-server.py

teach-selftest:
	@cd .. && python3 .agents/skills/teach/scripts/teach-server.py --selftest
