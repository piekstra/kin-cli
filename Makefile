BIN := kin
CARGO ?= cargo

.PHONY: build release test lint fmt fmt-check clean install deps smoke verify dev

# Every target that produces a binary signs it. `cargo build` and
# `cargo install` ad-hoc sign, minting a new code identity each time, and
# macOS scopes keychain "Always Allow" grants to that identity: an unsigned
# rebuild silently revokes the grant and the next run prompts again. The
# binary run most during development is ./target/release/kin, so `release`
# signs too, not only `install` and `dev`.
build: SIGN_TARGET = target/debug/$(BIN)
build:
	$(CARGO) build
	@$(SIGN)

release: SIGN_TARGET = target/release/$(BIN)
release:
	$(CARGO) build --release
	@$(SIGN)

test:
	$(CARGO) test

lint:
	$(CARGO) clippy --all-targets -- -D warnings

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

clean:
	$(CARGO) clean

deps:
	$(CARGO) fetch

# `--locked` builds from Cargo.lock, the versions the tests and audit ran on.
install: SIGN_TARGET = $${CARGO_INSTALL_ROOT:-$$HOME/.cargo}/bin/$(BIN)
install:
	$(CARGO) install --path . --locked --force
	@$(SIGN)

dev: SIGN_TARGET = target/debug/$(BIN)
dev:
	$(CARGO) build
	@$(SIGN)

# Offline smoke test: never touches the keychain, the network, or Kin.
smoke: build
	@./target/debug/$(BIN) --version >/dev/null
	@./target/debug/$(BIN) info --json >/dev/null
	@./target/debug/$(BIN) completions bash >/dev/null
	@./target/debug/$(BIN) policies list --help >/dev/null
	@./target/debug/$(BIN) policies get --help >/dev/null
	@./target/debug/$(BIN) documents list --help >/dev/null
	@./target/debug/$(BIN) documents download --help >/dev/null
	@./target/debug/$(BIN) payments list --help >/dev/null
	@./target/debug/$(BIN) auth login --help >/dev/null
	@echo "smoke ok"

verify: fmt-check lint test smoke

define SIGN
if [ -x "$$HOME/Dev/cli-common/scripts/dev-sign.sh" ]; then \
	"$$HOME/Dev/cli-common/scripts/dev-sign.sh" "$(SIGN_TARGET)"; \
else echo "cli-common/scripts/dev-sign.sh not found — $(SIGN_TARGET) left ad-hoc signed"; fi
endef
