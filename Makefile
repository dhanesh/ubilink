# hooky — build / test / run
#
# This is a no-build, dependency-light project: two standalone Rust crates
# (no workspace) plus a zero-npm vanilla frontend in dist/.
#   • src-tauri/  — hooky-core lib + the `hooky-sign` CLI
#   • registry/   — the `hooky-registry` service (separate crate; S2 keeps a server
#                   out of the core)
#   • dist/       — the panel UI (hand-authored ES modules; no bundler)
#
# NOTE on `run`: the full macOS desktop app needs the Tauri shell wired up
# (the `tauri` crate + #[tauri::command] wrappers + a global shortcut). That step
# is ENV-blocked and documented in docs/RUNBOOK-gui-panel.md — it is NOT built yet.
# Until then `make run` serves the panel UI for preview, and `make run-registry`
# runs the registry service. Both are real, runnable artifacts.

CARGO    ?= cargo
NODE     ?= node
PYTHON   ?= python3
CORE     := src-tauri/Cargo.toml
REGISTRY := registry/Cargo.toml
DESKTOP  := desktop/Cargo.toml
UI_DIR   := dist
UI_PORT  ?= 1420

.DEFAULT_GOAL := help
.PHONY: help build build-release build-desktop test test-rust test-js validate check \
        run run-desktop run-ui run-registry fmt clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-15s\033[0m %s\n",$$1,$$2}'

build: ## Build all crates (debug): core lib, hooky-sign, hooky-registry
	$(CARGO) build --manifest-path $(CORE)
	$(CARGO) build --manifest-path $(REGISTRY)

build-release: ## Build all crates optimized (release)
	$(CARGO) build --release --manifest-path $(CORE)
	$(CARGO) build --release --manifest-path $(REGISTRY)

build-desktop: ## Build the Tauri desktop app (heavy: pulls the Tauri dep tree)
	$(CARGO) build --manifest-path $(DESKTOP)

test: test-rust test-js ## Run every test suite (Rust core + registry, then JS panel)

test-rust: ## cargo test for the core and registry crates
	$(CARGO) test --manifest-path $(CORE)
	$(CARGO) test --manifest-path $(REGISTRY)

test-js: ## node --test for the panel view-state reducer (zero deps)
	$(NODE) --test '$(UI_DIR)/*.test.mjs'

validate: ## Validate the Manifold spec (needs the `manifold` CLI)
	manifold validate linking-manifesto-app

check: build test validate ## Full local verification: build + all tests + spec validate

run: run-desktop ## Run the local OS app (the Tauri desktop panel; needs a desktop session)

run-desktop: ## Build + launch the hooky desktop app (global hotkey: Cmd/Ctrl+Shift+K)
	@echo "Launching hooky. Summon the panel with Cmd/Ctrl+Shift+K."
	@echo "(Grant Automation access on first capture — see docs/RUNBOOK-gui-panel.md.)"
	$(CARGO) run --manifest-path $(DESKTOP)

run-ui: ## Browser-only preview of the panel UI at http://localhost:$(UI_PORT)
	@echo "Serving $(UI_DIR)/ at http://localhost:$(UI_PORT)  (Ctrl-C to stop)"
	@echo "NOTE: preview only — no Tauri bridge in a plain browser, so capture degrades"
	@echo "      to an explained error (U3). For the real app use 'make run'."
	@$(PYTHON) -c "import http.server, functools as f; \
h = http.server.SimpleHTTPRequestHandler; \
h.extensions_map['.mjs'] = 'text/javascript'; \
http.server.test(HandlerClass=f.partial(h, directory='$(UI_DIR)'), port=$(UI_PORT))"

run-registry: build ## Run the integration registry server (set HOOKY_TRUSTED_KEYS=<hex,...>)
	@echo "Starting hooky-registry on 127.0.0.1:8787 (set HOOKY_TRUSTED_KEYS to allow publishes)"
	./registry/target/debug/hooky-registry

fmt: ## Format Rust code in both crates
	$(CARGO) fmt --manifest-path $(CORE)
	$(CARGO) fmt --manifest-path $(REGISTRY)

clean: ## Remove build artifacts from both crates
	$(CARGO) clean --manifest-path $(CORE)
	$(CARGO) clean --manifest-path $(REGISTRY)
