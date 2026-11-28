# Common tasks. `make` alone lists them.

.DEFAULT_GOAL := help
.PHONY: help run build release app install test lint check render android android-run screenshots icons clean

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-12s %s\n", $$1, $$2}'

run: ## Run the desktop app (debug build)
	cargo run

build: ## Build the desktop app (debug)
	cargo build

release: ## Build the desktop app (release, target/release/diari)
	cargo build --release --locked

app: ## Build the macOS app bundle (target/release/diari.app)
	scripts/macos-app.sh

install: ## Build diari.app and install it in /Applications
	scripts/macos-app.sh install

test: ## Run the tests
	cargo test

lint: ## Run clippy
	cargo clippy --all-targets

check: test lint ## Tests and clippy

render: ## Render the app offscreen to PNGs in target/render (no screen needed)
	mkdir -p target/render
	cargo run --example render_editor -- target/render

android: ## Build the Android APK (target/release/apk/diari.apk)
	scripts/android.sh build

android-run: ## Build, install and launch the APK on the connected device
	scripts/android.sh run

screenshots: ## Retake the README screenshot (macOS, demo notes)
	scripts/screenshots.sh macos

icons: ## Render the app icons from icons/*.svg
	scripts/icons.sh

clean: ## Remove build output
	cargo clean
