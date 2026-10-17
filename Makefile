# GNU make. Windows: `winget install ezwinports.make`, then run from Git Bash.
SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
CARGO ?= cargo

.PHONY: lint test e2e smoke battery battery-setup golden check build

lint:
	$(CARGO) fmt -- --check
	$(CARGO) clippy --locked --all-targets -- -D warnings

test:
	$(CARGO) test --locked --bins

# The battery's own tests run here too, against the fake; its live runs are ignored.
e2e:
	$(CARGO) test --locked --test e2e --test battery

# Real API, real (tiny) spend; never in CI.
smoke:
	@test -n "$${CROWBOT_SMOKE_KEY:-}" || { echo "set CROWBOT_SMOKE_KEY (smoke spends real money)"; exit 1; }
	$(CARGO) test --locked --test smoke -- --ignored --test-threads=1

# The tool-use battery (bench/, NEXT_PLAN.md M9 and M11); never in CI. Setup clones the repos and
# runs their code; the battery spends real money. `make battery TASKS=a,b REPEATS=1` runs a pilot;
# `SUITE=mcp` the MCP suite, `MODEL=<id>` another model than the suite's.
battery-setup:
	$(CARGO) test --locked --test battery setup -- --ignored --exact --nocapture

battery:
	@test -n "$${CROWBOT_SMOKE_KEY:-}" || { echo "set CROWBOT_SMOKE_KEY (the battery spends real money)"; exit 1; }
	BATTERY_SUITE="$(SUITE)" BATTERY_MODEL="$(MODEL)" BATTERY_TASKS="$(TASKS)" BATTERY_REPEATS="$(REPEATS)" $(CARGO) test --locked --test battery live -- --ignored --exact --nocapture

# Rewrites golden files and insta snapshots from current output; only for a deliberate change.
golden:
	UPDATE_GOLDEN=1 INSTA_UPDATE=always $(CARGO) test --locked --bins
	UPDATE_GOLDEN=1 $(CARGO) test --locked --test e2e

check: lint test e2e

# `make build TARGET=<triple>` builds for another target, e.g. static musl in CI. Only `crowbot`
# ships; the e2e suite's fake MCP server is built by the tests that run it.
build:
	$(CARGO) build --locked --release --bin crowbot $(if $(TARGET),--target $(TARGET))
