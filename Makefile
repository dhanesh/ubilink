# ubilink — build / test / run
#
# Four standalone Rust crates (no workspace):
#   • core/         — ubilink-core lib + the `ubilink-sign` CLI (GUI-free; named
#                     `src-tauri/` until D25)
#   • registry/     — the `ubilink-registry` service (separate crate; S2 keeps a server
#                     out of the core)
#   • app/          — ubilink-app, the GUI-free command layer the desktop app calls (R3);
#                     no tauri*/gpui* crate in its tree (verify-deps-clean)
#   • desktop-gpui/ — the macOS app itself: native GPUI, binary `ubilink`, Ubilink.app,
#                     org.dhanesh.ubilink (D24 — it replaced the Tauri app in Phase 2b)
#
# `make bundle` builds Ubilink.app for local use and `make run` opens it; it runs
# against your REAL data dir (~/Library/Application Support/org.dhanesh.ubilink) and
# needs a desktop session + the Automation grant (docs/RUNBOOK-gui-panel.md).
# `make run-registry` runs the registry service. The automation socket the probes use
# never reaches a release build (release-clean, part of `make verify`). `make probe`
# drives the app with scripts/probe.py's GUI scenarios for local use; those need a
# desktop session, so they are NOT part of `make verify` (R16).

CARGO    ?= cargo
# CI sets LOCKED=--locked so build/test/clippy pin to the committed lockfiles
# (reproducible, no silent dep drift). Empty locally for dev ergonomics.
LOCKED   ?=
PYTHON   ?= python3
CORE     := core/Cargo.toml
REGISTRY := registry/Cargo.toml
# R3 (GPUI Phase 0 plan, task 3): the GUI-free command layer the desktop app calls.
# Lightweight like CORE/REGISTRY (no tauri*/gpui* in its tree — see
# verify-deps-clean), so it folds into the same fast targets they use.
APP      := app/Cargo.toml
# R2: the GPUI app crate. Heavy (pulls gpui-kit + objc2 + tray-icon) and needs full
# Xcode for its Metal shaders, so it gets its own build-gpui/test-gpui targets
# instead of folding into the fast build/test loop.
NEXT     := desktop-gpui/Cargo.toml
# The app's real data dir (desktop-gpui/src/paths.rs); `make run` uses it.
DATA_DIR := $(HOME)/Library/Application Support/org.dhanesh.ubilink

.DEFAULT_GOAL := help
.PHONY: help build build-release build-gpui bundle \
        test test-rust test-gpui \
        validate verify verify-fmt verify-clippy verify-spec verify-deps-clean check \
        verify-version test-scripts release-clean budget dist \
        run run-registry probe fmt clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-15s\033[0m %s\n",$$1,$$2}'

build: ## Build the light crates (debug): core lib, ubilink-sign, ubilink-registry, ubilink-app
	$(CARGO) build $(LOCKED) --manifest-path $(CORE)
	$(CARGO) build $(LOCKED) --manifest-path $(REGISTRY)
	$(CARGO) build $(LOCKED) --manifest-path $(APP)

build-release: ## Build the light crates optimized (release)
	$(CARGO) build $(LOCKED) --release --manifest-path $(CORE)
	$(CARGO) build $(LOCKED) --release --manifest-path $(REGISTRY)
	$(CARGO) build $(LOCKED) --release --manifest-path $(APP)

build-gpui: ## Build the GPUI app, desktop-gpui/ (debug; heavy: pulls the GPUI dep tree; needs full Xcode)
	$(CARGO) build $(LOCKED) --manifest-path $(NEXT)

bundle: ## Build Ubilink.app (GPUI) into desktop-gpui/bundle/ for local use (stable self-signed identity so macOS keeps permission grants)
	bash scripts/bundle-macos.sh dev

dist: ## Universal DMG: notarized when all six APPLE_* credentials are set, else an UNNOTARIZED dry run
	@mode="$$(bash scripts/release-mode.sh "$${GITHUB_REF:-}")" && bash scripts/bundle-macos.sh "$$mode"

test: test-rust test-scripts ## Run the fast suites (Rust core + registry + app, script tests)

test-rust: ## cargo test for the core, registry and app crates
	$(CARGO) test $(LOCKED) --manifest-path $(CORE)
	$(CARGO) test $(LOCKED) --manifest-path $(REGISTRY)
	$(CARGO) test $(LOCKED) --manifest-path $(APP)

test-scripts: ## Run the script tests (scripts/test-*.sh), probe.py's self-tests, and the fixtures schema check
	@set -e; for t in scripts/test-*.sh; do bash "$$t"; done
	@# R8 (GPUI Phase 0): the probe scenario engine's headless self-tests — stdlib
	@# python, no app build or launch, so they belong with the fast script tests.
	$(PYTHON) -m unittest scripts/test_probe.py
	@# R10: fixtures must keep matching desktop-gpui/probes/fixtures/README.md's
	@# documented schema on every gate run, not just when they're (re)generated.
	$(PYTHON) scripts/probe.py fixtures --check

test-gpui: ## cargo test for the GPUI app (R16), without and with --features automation
	$(CARGO) test $(LOCKED) --manifest-path $(NEXT)
	$(CARGO) test $(LOCKED) --manifest-path $(NEXT) --features automation

validate: ## Validate the Manifold spec (needs the `manifold` CLI)
	manifold validate linking-manifesto-app

# ── The verifier (R1): ONE command that proves a change is sound. fmt-check +
# clippy(-D warnings) + all tests across EVERY crate (core, registry, app, GPUI),
# the script tests, plus the release-clean automation-socket guard (R16) and
# optional spec validation. This is the merge gate — CI runs the SAME target (with
# LOCKED=--locked), so "green locally" means "green in CI".
verify: verify-fmt verify-clippy verify-version verify-deps-clean test test-gpui release-clean verify-spec ## Full agent/CI gate: fmt+lint+version+test all crates (incl. GPUI) + script tests + release-clean + spec
	@echo "[verify] all checks passed ✓"

verify-fmt: ## Fail if any crate is unformatted (non-mutating; run 'make fmt' to fix)
	$(CARGO) fmt --manifest-path $(CORE) --check
	$(CARGO) fmt --manifest-path $(REGISTRY) --check
	$(CARGO) fmt --manifest-path $(APP) --check
	$(CARGO) fmt --manifest-path $(NEXT) --check

verify-clippy: ## Lint every crate; warnings are errors
	$(CARGO) clippy $(LOCKED) --manifest-path $(CORE) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(REGISTRY) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(APP) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(NEXT) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(NEXT) --all-targets --features automation -- -D warnings

verify-version: ## Fail if crate versions or CHANGELOG.md disagree with desktop-gpui/Cargo.toml (the app version)
	bash scripts/check-version.sh

verify-deps-clean: ## R3: ubilink-app's dependency tree must carry no tauri*/gpui*/wry/tao/webkit* crate
	$(PYTHON) scripts/probe.py deps-clean $(APP)

verify-spec: ## Validate the Manifold spec IF the CLI is present (skips cleanly in a bare env)
	@if command -v manifold >/dev/null 2>&1; then \
	  manifold validate linking-manifesto-app; \
	else \
	  echo "[verify] manifold CLI not found — skipping spec validation (not a failure)"; \
	fi

check: verify ## Alias for 'verify' (the full gate)

# R6: the shipped GPUI release binary must carry no automation socket path or
# verb names (the automation feature is dev/probe-only — see desktop-gpui/
# src/automation and S2's two-socket separation). Part of `make verify` because,
# unlike the GUI probes below, it only builds binaries — no desktop session.
release-clean: ## Fail if the desktop-gpui release binary leaks the automation socket or verbs (R6/R16)
	$(PYTHON) scripts/probe.py release-clean

# R15 (Phase 1 records this budget; Phase 0 ships the target — ruling R-s/I5).
# NOT part of `make verify`: it launches the real release app and measures idle
# CPU/footprint/hotkey latency on a live desktop session, like `make probe`.
budget: ## Measure the release app (idle CPU, footprint, binary size, hotkey latency); writes docs/PERFORMANCE.md
	$(PYTHON) scripts/probe.py budget --write docs/PERFORMANCE.md

# `open` launches through LaunchServices, which passes no shell environment to the
# app, so UBILINK_DATA_DIR cannot redirect this run: it is always the real data dir.
# Probes and the verify skill launch isolated instances instead.
run: bundle ## Build Ubilink.app (make bundle) and open it — uses your REAL data dir (~/Library/Application Support/org.dhanesh.ubilink)
	@echo "Opening desktop-gpui/bundle/Ubilink.app against your REAL data dir:"
	@echo "  $(DATA_DIR)"
	@echo "Summon the panel with Cmd+Shift+K. Grant Automation on first capture (docs/RUNBOOK-gui-panel.md)."
	open desktop-gpui/bundle/Ubilink.app

run-registry: build ## Run the integration registry server (set UBILINK_TRUSTED_KEYS=<hex,...>)
	@echo "Starting ubilink-registry on 127.0.0.1:8787 (set UBILINK_TRUSTED_KEYS to allow publishes)"
	./registry/target/debug/ubilink-registry

# R8/R9: GUI probes drive a real automation build against a desktop session
# (Screen Recording + Automation grants) — they cannot run in CI or in
# `make verify`. `doctor` preflights those grants before `run` so a missing
# grant fails fast with a clear reason instead of a confusing probe timeout.
probe: ## Run the local GPUI probes against a desktop session (doctor preflight, then shell/panel-hotkey/panel-dismiss/a11y-spike)
	$(PYTHON) scripts/probe.py doctor && $(PYTHON) scripts/probe.py run shell panel-hotkey panel-dismiss a11y-spike

fmt: ## Format Rust code in every crate
	$(CARGO) fmt --manifest-path $(CORE)
	$(CARGO) fmt --manifest-path $(REGISTRY)
	$(CARGO) fmt --manifest-path $(APP)
	$(CARGO) fmt --manifest-path $(NEXT)

clean: ## Remove build artifacts from every crate
	$(CARGO) clean --manifest-path $(CORE)
	$(CARGO) clean --manifest-path $(REGISTRY)
	$(CARGO) clean --manifest-path $(APP)
	$(CARGO) clean --manifest-path $(NEXT)
