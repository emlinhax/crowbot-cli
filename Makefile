# GNU make. Windows: `winget install ezwinports.make`, then run from Git Bash.
SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
CARGO ?= cargo

.PHONY: lint test e2e smoke golden check build

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -- -D warnings

test:
	$(CARGO) test --locked --bins

e2e:
	$(CARGO) test --locked --test e2e

# Real API, real (tiny) spend; never in CI.
smoke:
	@test -n "$${CROWBOT_SMOKE_KEY:-}" || { echo "set CROWBOT_SMOKE_KEY (smoke spends real money)"; exit 1; }
	$(CARGO) test --locked --test smoke -- --ignored --test-threads=1

# Rewrites golden expectations from current output; only for a deliberate behaviour change.
golden:
	UPDATE_GOLDEN=1 $(CARGO) test --locked --bins golden

check: lint test e2e

build:
	$(CARGO) build --locked --release
