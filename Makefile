# Engram — common developer tasks.
# `make` with no target prints this help.

.DEFAULT_GOAL := help
.PHONY: help build release run test fmt lint bench eval check desktop clean install

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

lint: ## Clippy with warnings denied
	cargo clippy --workspace --all-targets -- -D warnings

bench: ## Paraphrase recall + footprint benchmark
	cargo run -p engram-bench

eval: ## Deterministic agent regression suite
	cargo run -p engram-eval

check: fmt lint test eval ## Everything CI runs, locally

desktop: ## Build & launch the native desktop app (needs tauri-cli ^2)
	scripts/desktop.sh

install: ## Install engramd + engram into ~/.cargo/bin
	cargo install --path crates/engramd --locked
	cargo install --path crates/engram-cli --locked

clean: ## Remove build artifacts
	cargo clean
