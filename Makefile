# Engram — common developer tasks.
# `make` with no target prints this help.

.DEFAULT_GOAL := help
.PHONY: help build release run test fmt fmt-check lint lint-all test-all bench eval check ci desktop clean install

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-10s\033[0m %s\n", $$1, $$2}'

build: ## Debug build of the whole workspace
	cargo build --workspace

release: ## Optimized release build
	cargo build --release

run: ## Run the daemon (http://127.0.0.1:8088)
	cargo run -p engramd

test: ## Run the full test suite
	cargo test --workspace

fmt: ## Format the code
	cargo fmt --all

fmt-check: ## Fail if any file needs formatting (what CI runs)
	cargo fmt --all --check

lint: ## Clippy with warnings denied
	cargo clippy --workspace --all-targets -- -D warnings

lint-all: lint ## Clippy across every feature combination CI checks
	cargo clippy -p engramd --features http --all-targets -- -D warnings
	cargo clippy -p engramd --features http,docs,keyring --all-targets -- -D warnings

test-all: test ## Tests across every feature combination CI runs
	cargo test -p engramd --features http
	cargo test -p engramd --features http,docs
	cargo test -p engram-agent --features browser-cdp

bench: ## Paraphrase recall + footprint benchmark
	cargo run -p engram-bench

eval: ## Deterministic agent regression suite
	cargo run -p engram-eval

check: fmt lint test eval ## Quick local gate: format, lint, test, eval

ci: fmt-check lint-all test-all eval ## The full CI matrix, locally (run before pushing)

desktop: ## Build & launch the native desktop app (needs tauri-cli ^2)
	scripts/desktop.sh

install: ## Install engramd + engram into ~/.cargo/bin (real providers + document ingest)
	cargo install --path crates/engramd --locked --features http,docs
	cargo install --path crates/engram-cli --locked

clean: ## Remove build artifacts
	cargo clean
