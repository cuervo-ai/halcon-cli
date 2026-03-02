.PHONY: wasm-rebuild wasm-verify build test test-color-science submodule-init

# ── WASM targets ─────────────────────────────────────────────────────────────
## Rebuild the Momoto UI Core WASM binary and sync to website/
wasm-rebuild:
	./scripts/wasm-rebuild.sh

## Verify WASM checksums match source (no rebuild)
wasm-verify:
	./scripts/wasm-rebuild.sh --verify

# ── Submodule ─────────────────────────────────────────────────────────────────
## Initialize or update the momoto-ui submodule
submodule-init:
	git submodule update --init --recursive vendor/momoto-ui

# ── Build targets ─────────────────────────────────────────────────────────────
## Build with all features (requires momoto submodule)
build:
	cargo build -p halcon-cli

## Build without color-science (CI-safe, no submodule required)
build-ci:
	cargo build -p halcon-cli --no-default-features --features tui

## Build release with color-science
build-release:
	cargo build --release -p halcon-cli

# ── Test targets ─────────────────────────────────────────────────────────────
## Run all tests without color-science (CI-safe)
test:
	cargo test --workspace --no-default-features

## Run color-science tests (requires momoto submodule)
test-color-science:
	cargo test -p halcon-cli --features color-science --lib

## Run delta-E palette validation specifically
test-delta-e:
	cargo test -p halcon-cli --features color-science --lib \
		tui_colors_perceptually_distinct_neon panel_sections_distinguishable \
		toast_levels_distinguishable -- --nocapture

## Run full test suite with both feature sets
test-all: test test-color-science

# ── Install ───────────────────────────────────────────────────────────────────
## Install release binary to ~/.local/bin/halcon
install:
	cargo build --release -p halcon-cli
	cp target/release/halcon ~/.local/bin/halcon
	codesign --sign - --force ~/.local/bin/halcon
	@echo "✅ Installed ~/.local/bin/halcon"

# ── Cross-compilation (Linux targets via cross+Docker) ────────────────────────
## Linux x86_64 static musl (broadest compatibility)
build-linux-x86:
	./scripts/build-cross.sh x86_64-unknown-linux-musl --release

## Linux ARM64 dynamic glibc
build-linux-arm64:
	./scripts/build-cross.sh aarch64-unknown-linux-gnu --release

## Linux ARM64 static musl (Alpine/Docker)
build-linux-arm64-musl:
	./scripts/build-cross.sh aarch64-unknown-linux-musl --release

## All Linux targets
build-linux-all: build-linux-x86 build-linux-arm64

## Show contents of dist/
dist-list:
	@ls -lh dist/ 2>/dev/null || echo "(dist/ is empty)"
