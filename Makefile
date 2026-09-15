# ubilink — build / test / run
#
# This is a no-build, dependency-light project: two standalone Rust crates
# (no workspace) plus a zero-npm vanilla frontend in dist/.
#   • src-tauri/  — ubilink-core lib + the `ubilink-sign` CLI
#   • registry/   — the `ubilink-registry` service (separate crate; S2 keeps a server
#                   out of the core)
#   • dist/       — the panel UI (hand-authored ES modules; no bundler)
#
# The macOS desktop app IS wired up (desktop/ — the Tauri shell, #[tauri::command]
# wrappers, global hotkey). `make run` builds and launches it (needs a desktop
# session + Accessibility/Automation grants — see docs/RUNBOOK-gui-panel.md).
# `make run-ui` serves the panel UI in a plain browser for quick preview, and
# `make run-registry` runs the registry service.

CARGO    ?= cargo
# CI sets LOCKED=--locked so build/test/clippy pin to the committed lockfiles
# (reproducible, no silent dep drift). Empty locally for dev ergonomics.
LOCKED   ?=
NODE     ?= node
PYTHON   ?= python3
CORE     := src-tauri/Cargo.toml
REGISTRY := registry/Cargo.toml
DESKTOP  := desktop/Cargo.toml
UI_DIR   := dist
UI_PORT  ?= 1420

.DEFAULT_GOAL := help
.PHONY: help build build-release build-desktop bundle test test-rust test-js test-desktop \
        validate verify verify-fmt verify-clippy verify-spec check \
        verify-version test-scripts dist \
        run run-desktop run-ui run-registry fmt clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-15s\033[0m %s\n",$$1,$$2}'

build: ## Build all crates (debug): core lib, ubilink-sign, ubilink-registry
	$(CARGO) build $(LOCKED) --manifest-path $(CORE)
	$(CARGO) build $(LOCKED) --manifest-path $(REGISTRY)

build-release: ## Build all crates optimized (release)
	$(CARGO) build $(LOCKED) --release --manifest-path $(CORE)
	$(CARGO) build $(LOCKED) --release --manifest-path $(REGISTRY)

build-desktop: ## Build the Tauri desktop app (heavy: pulls the Tauri dep tree)
	$(CARGO) build $(LOCKED) --manifest-path $(DESKTOP)

bundle: ## Build a signed macOS ubilink.app (stable path so TCC grants persist)
	bash scripts/bundle-macos.sh

test: test-rust test-js test-scripts ## Run the fast suites (Rust core + registry, JS panel, script tests)

test-rust: ## cargo test for the core and registry crates
	$(CARGO) test $(LOCKED) --manifest-path $(CORE)
	$(CARGO) test $(LOCKED) --manifest-path $(REGISTRY)

test-js: ## node --test for the panel view-state reducer (zero deps)
	@# node --test exits 0 when the glob matches nothing — fail loudly instead.
	@set -- $(UI_DIR)/*.test.mjs; [ -e "$$1" ] || { echo "test-js: no $(UI_DIR)/*.test.mjs found" >&2; exit 1; }
	$(NODE) --test '$(UI_DIR)/*.test.mjs'

test-scripts: ## Run the shell tests for the release scripts (scripts/test-*.sh)
	@set -e; for t in scripts/test-*.sh; do bash "$$t"; done

test-desktop: ## cargo test for the Tauri desktop crate (heavy: pulls the Tauri dep tree)
	$(CARGO) test $(LOCKED) --manifest-path $(DESKTOP)

validate: ## Validate the Manifold spec (needs the `manifold` CLI)
	manifold validate linking-manifesto-app

# ── The verifier (R1): ONE command that proves a change is sound. fmt-check +
# clippy(-D warnings) + all tests across EVERY crate (core, registry, desktop) and
# the JS panel + optional spec validation. This is the merge gate — CI runs the
# SAME target (with LOCKED=--locked), so "green locally" means "green in CI".
verify: verify-fmt verify-clippy verify-version test test-desktop verify-spec ## Full agent/CI gate: fmt+lint+version+test all crates + JS + spec
	@echo "[verify] all checks passed ✓"

verify-fmt: ## Fail if any crate is unformatted (non-mutating; run 'make fmt' to fix)
	$(CARGO) fmt --manifest-path $(CORE) --check
	$(CARGO) fmt --manifest-path $(REGISTRY) --check
	$(CARGO) fmt --manifest-path $(DESKTOP) --check

verify-clippy: ## Lint every crate; warnings are errors
	$(CARGO) clippy $(LOCKED) --manifest-path $(CORE) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(REGISTRY) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(DESKTOP) --all-targets -- -D warnings

verify-version: ## Fail if crate versions, tauri.conf.json or CHANGELOG.md disagree with desktop/Cargo.toml
	bash scripts/check-version.sh

verify-spec: ## Validate the Manifold spec IF the CLI is present (skips cleanly in a bare env)
	@if command -v manifold >/dev/null 2>&1; then \
	  manifold validate linking-manifesto-app; \
	else \
	  echo "[verify] manifold CLI not found — skipping spec validation (not a failure)"; \
	fi

check: verify ## Alias for 'verify' (the full gate)

run: run-desktop ## Run the local OS app (the Tauri desktop panel; needs a desktop session)

run-desktop: ## Build + launch the ubilink desktop app (global hotkey: Cmd+Shift+K)
	@echo "Launching Ubilink. Summon the panel with Cmd+Shift+K."
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

run-registry: build ## Run the integration registry server (set UBILINK_TRUSTED_KEYS=<hex,...>)
	@echo "Starting ubilink-registry on 127.0.0.1:8787 (set UBILINK_TRUSTED_KEYS to allow publishes)"
	./registry/target/debug/ubilink-registry

fmt: ## Format Rust code in every crate
	$(CARGO) fmt --manifest-path $(CORE)
	$(CARGO) fmt --manifest-path $(REGISTRY)
	$(CARGO) fmt --manifest-path $(DESKTOP)

clean: ## Remove build artifacts from every crate
	$(CARGO) clean --manifest-path $(CORE)
	$(CARGO) clean --manifest-path $(REGISTRY)
	$(CARGO) clean --manifest-path $(DESKTOP)
