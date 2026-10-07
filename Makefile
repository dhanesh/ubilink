# ubilink — build / test / run
#
# This is a no-build, dependency-light project: two standalone Rust crates
# (no workspace) plus a zero-npm vanilla frontend in dist/.
#   • src-tauri/  — ubilink-core lib + the `ubilink-sign` CLI
#   • registry/   — the `ubilink-registry` service (separate crate; S2 keeps a server
#                   out of the core)
#   • app/        — ubilink-app, the GUI-free command layer both desktop shells call
#                   (R3); no tauri*/gpui* crate in its tree (verify-deps-clean).
#   • dist/       — the panel UI (hand-authored ES modules; no bundler)
#
# The macOS desktop app IS wired up (desktop/ — the Tauri shell, #[tauri::command]
# wrappers, global hotkey). `make run` builds and launches it (needs a desktop
# session + Accessibility/Automation grants — see docs/RUNBOOK-gui-panel.md).
# `make run-ui` serves the panel UI in a plain browser for quick preview, and
# `make run-registry` runs the registry service.
#
# desktop-gpui/ (`ubilink-next.app`, org.ubilink.next) is the GPUI Phase 0 shell
# (R2): it lives beside desktop/ rather than replacing it, builds/tests via
# build-next/bundle-next/test-gpui, and keeps its automation socket out of release
# builds (release-clean). `make probe` drives it with scripts/probe.py's GUI
# scenarios for local use; those need a desktop session, so they are NOT part of
# `make verify` (R16).

CARGO    ?= cargo
# CI sets LOCKED=--locked so build/test/clippy pin to the committed lockfiles
# (reproducible, no silent dep drift). Empty locally for dev ergonomics.
LOCKED   ?=
NODE     ?= node
PYTHON   ?= python3
CORE     := src-tauri/Cargo.toml
REGISTRY := registry/Cargo.toml
DESKTOP  := desktop/Cargo.toml
# R3 (GPUI Phase 0 plan, task 3): the GUI-free Tauri/GPUI command layer shared by
# both desktop shells. Lightweight like CORE/REGISTRY (no tauri*/gpui* in its
# tree — see verify-deps-clean), so it folds into the same fast targets they use
# rather than getting its own heavy test-desktop-style target.
APP      := app/Cargo.toml
# R2: the GPUI shell crate. Heavy (pulls gpui-kit + objc2 + tray-icon) and needs
# full Xcode for its Metal shaders, so — like DESKTOP — it gets its own
# build-next/test-gpui targets instead of folding into build/test.
NEXT     := desktop-gpui/Cargo.toml
UI_DIR   := dist
UI_PORT  ?= 1420

.DEFAULT_GOAL := help
.PHONY: help build build-release build-desktop build-next bundle bundle-next \
        test test-rust test-js test-desktop test-gpui \
        validate verify verify-fmt verify-clippy verify-spec verify-deps-clean check \
        verify-version test-scripts release-clean budget dist \
        run run-desktop run-ui run-registry probe fmt clean

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
	  | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-15s\033[0m %s\n",$$1,$$2}'

build: ## Build all crates (debug): core lib, ubilink-sign, ubilink-registry, ubilink-app
	$(CARGO) build $(LOCKED) --manifest-path $(CORE)
	$(CARGO) build $(LOCKED) --manifest-path $(REGISTRY)
	$(CARGO) build $(LOCKED) --manifest-path $(APP)

build-release: ## Build all crates optimized (release)
	$(CARGO) build $(LOCKED) --release --manifest-path $(CORE)
	$(CARGO) build $(LOCKED) --release --manifest-path $(REGISTRY)
	$(CARGO) build $(LOCKED) --release --manifest-path $(APP)

build-desktop: ## Build the Tauri desktop app (heavy: pulls the Tauri dep tree)
	$(CARGO) build $(LOCKED) --manifest-path $(DESKTOP)

build-next: ## Build the GPUI shell, desktop-gpui/ (heavy: pulls the GPUI dep tree; needs full Xcode)
	$(CARGO) build $(LOCKED) --manifest-path $(NEXT)

bundle: ## Build Ubilink.app for local use (stable self-signed identity so macOS keeps permission grants)
	bash scripts/bundle-macos.sh dev

bundle-next: ## Build ubilink-next.app (GPUI shell, org.ubilink.next) for local use, same stable dev identity
	bash scripts/bundle-next.sh dev

dist: ## Universal DMG: notarized when all six APPLE_* credentials are set, else an UNNOTARIZED dry run
	@mode="$$(bash scripts/release-mode.sh "$${GITHUB_REF:-}")" && bash scripts/bundle-macos.sh "$$mode"

test: test-rust test-js test-scripts ## Run the fast suites (Rust core + registry + app, JS panel, script tests)

test-rust: ## cargo test for the core, registry and app crates
	$(CARGO) test $(LOCKED) --manifest-path $(CORE)
	$(CARGO) test $(LOCKED) --manifest-path $(REGISTRY)
	$(CARGO) test $(LOCKED) --manifest-path $(APP)

test-js: ## node --test for the panel view-state reducer (zero deps)
	@# node --test exits 0 when the glob matches nothing — fail loudly instead.
	@set -- $(UI_DIR)/*.test.mjs; [ -e "$$1" ] || { echo "test-js: no $(UI_DIR)/*.test.mjs found" >&2; exit 1; }
	$(NODE) --test '$(UI_DIR)/*.test.mjs'

test-scripts: ## Run the script tests (scripts/test-*.sh), probe.py's self-tests, and the fixtures schema check
	@set -e; for t in scripts/test-*.sh; do bash "$$t"; done
	@# R8 (GPUI Phase 0): the probe scenario engine's headless self-tests — stdlib
	@# python, no app build or launch, so they belong with the fast script tests.
	$(PYTHON) -m unittest scripts/test_probe.py
	@# R10: fixtures must keep matching desktop-gpui/probes/fixtures/README.md's
	@# documented schema on every gate run, not just when they're (re)generated.
	$(PYTHON) scripts/probe.py fixtures --check

test-desktop: ## cargo test for the Tauri desktop crate (heavy: pulls the Tauri dep tree)
	$(CARGO) test $(LOCKED) --manifest-path $(DESKTOP)

test-gpui: ## cargo test for the GPUI shell (R16), without and with --features automation
	$(CARGO) test $(LOCKED) --manifest-path $(NEXT)
	$(CARGO) test $(LOCKED) --manifest-path $(NEXT) --features automation

validate: ## Validate the Manifold spec (needs the `manifold` CLI)
	manifold validate linking-manifesto-app

# ── The verifier (R1): ONE command that proves a change is sound. fmt-check +
# clippy(-D warnings) + all tests across EVERY crate (core, registry, desktop,
# app, GPUI) and the JS panel, plus the release-clean automation-socket guard
# (R16) and optional spec validation. This is the merge gate — CI runs the SAME
# target (with LOCKED=--locked), so "green locally" means "green in CI".
verify: verify-fmt verify-clippy verify-version verify-deps-clean test test-desktop test-gpui release-clean verify-spec ## Full agent/CI gate: fmt+lint+version+test all crates (incl. GPUI) + JS + release-clean + spec
	@echo "[verify] all checks passed ✓"

verify-fmt: ## Fail if any crate is unformatted (non-mutating; run 'make fmt' to fix)
	$(CARGO) fmt --manifest-path $(CORE) --check
	$(CARGO) fmt --manifest-path $(REGISTRY) --check
	$(CARGO) fmt --manifest-path $(DESKTOP) --check
	$(CARGO) fmt --manifest-path $(APP) --check
	$(CARGO) fmt --manifest-path $(NEXT) --check

verify-clippy: ## Lint every crate; warnings are errors
	$(CARGO) clippy $(LOCKED) --manifest-path $(CORE) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(REGISTRY) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(DESKTOP) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(APP) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(NEXT) --all-targets -- -D warnings
	$(CARGO) clippy $(LOCKED) --manifest-path $(NEXT) --all-targets --features automation -- -D warnings

verify-version: ## Fail if crate versions, tauri.conf.json or CHANGELOG.md disagree with desktop/Cargo.toml
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

# R8/R9: GUI probes drive a real automation build against a desktop session
# (Screen Recording + Automation grants) — they cannot run in CI or in
# `make verify`. `doctor` preflights those grants before `run` so a missing
# grant fails fast with a clear reason instead of a confusing probe timeout.
probe: ## Run the local GPUI probes against a desktop session (doctor preflight, then shell/panel-hotkey/panel-dismiss/a11y-spike)
	$(PYTHON) scripts/probe.py doctor && $(PYTHON) scripts/probe.py run shell panel-hotkey panel-dismiss a11y-spike

fmt: ## Format Rust code in every crate
	$(CARGO) fmt --manifest-path $(CORE)
	$(CARGO) fmt --manifest-path $(REGISTRY)
	$(CARGO) fmt --manifest-path $(DESKTOP)
	$(CARGO) fmt --manifest-path $(APP)
	$(CARGO) fmt --manifest-path $(NEXT)

clean: ## Remove build artifacts from every crate
	$(CARGO) clean --manifest-path $(CORE)
	$(CARGO) clean --manifest-path $(REGISTRY)
	$(CARGO) clean --manifest-path $(DESKTOP)
	$(CARGO) clean --manifest-path $(APP)
	$(CARGO) clean --manifest-path $(NEXT)
