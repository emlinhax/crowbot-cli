# GNU make. Windows: `winget install ezwinports.make`, then run from Git Bash.
SHELL := bash
.SHELLFLAGS := -eu -o pipefail -c
CARGO ?= cargo

.PHONY: lint test e2e check build

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -- -D warnings

test:
	$(CARGO) test --locked --bins

e2e:
	$(CARGO) test --locked --test e2e

check: lint test e2e

build:
	$(CARGO) build --locked --release
