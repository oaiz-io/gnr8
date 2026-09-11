# gnr8 quality gates (D-16 + Go fixture gate).
#
# `make check` is the full LOCAL gate. CI splits the same confidence areas into bounded jobs. It runs
# fmt-check, clippy
# (--locked, -D warnings), Rust tests, sidecar tests, fixture/helper builds, and example regen checks.
#
# The Go/Python/TypeScript contract snapshots are GREEN and blocking. `make gates` runs the Rust
# contract set; `make check` adds direct sidecar tests and release-example regeneration.

.PHONY: fmt fmt-check clippy test gates fixture-build goextract-build pyextract-test tsextract-deps tsextract-test action-test action-annotations-test release-notes-test red check all examples-check install invariants

# Auto-format the workspace in place.
fmt:
	cargo fmt --all

# Verify formatting without modifying files (CI-equivalent).
fmt-check:
	cargo fmt --all -- --check

# Build the host release archive and install its complete binary + resource layout.
install:
	@set -e; \
	case "$$(uname -s)" in \
	  Darwin) asset_os=macos ;; \
	  Linux) asset_os=linux ;; \
	  *) echo "make install: unsupported OS: $$(uname -s)" >&2; exit 1 ;; \
	esac; \
	case "$$(uname -m)" in \
	  x86_64|amd64) asset_arch=x86_64 ;; \
	  arm64|aarch64) asset_arch=aarch64 ;; \
	  *) echo "make install: unsupported architecture: $$(uname -m)" >&2; exit 1 ;; \
	esac; \
	target="$$(rustc -vV | sed -n 's/^host: //p')"; \
	archive="$$(TARGET="$$target" ASSET_OS="$$asset_os" ASSET_ARCH="$$asset_arch" scripts/package-release.sh)"; \
	GNR8_ARCHIVE="$$archive" scripts/install.sh

# Lint with warnings denied; --locked requires a committed, up-to-date Cargo.lock (Pitfall 4).
clippy:
	cargo clippy --all-targets --all-features --locked -- -D warnings

# Full Rust test suite.
test:
	cargo test --all-features

# Blocking gate test set: green unit + CLI parse tests (incl. the pure `watch::tests` loop-safety
# filter tests, the host→worker→write `generate_e2e` integration test, and the `worker_contract`
# host/worker boundary suite in `cargo test -p gnr8-cli`), the thin-SDK boundary + protocol tests in
# `cargo test -p gnr8`, ALL
# FOUR contract tests (snapshot_graph/diagnostics/openapi/sdk), determinism (graph + OpenAPI + SDK
# byte-identical), sdk_compile (temp dir + zero-require go.mod + go build + httptest smoke, SDK-05),
# pysdk_compile (temp dir + bookstore package + py_compile + import + stdlib http.server round-trip:
# 2xx dataclass + 4xx typed ApiError via an injected OpenerDirector, PYSDK-02 — actually RUNS here since
# python3 is present), tssdk_compile (temp dir + generate the TS SDK + the hermetic
# `tsc --noEmit --strict --lib es2022,dom` typecheck via the dev-installed typescript (`make tsextract-deps`;
# in real use the user's own project typescript) + a banned-import grep,
# TSSDK-02 — actually RUNS here since node + typescript are present), the `sdk_pipeline`
# SDK-framework integration test, and the `lifecycle` suite
# (manifest round-trip + the
# pure `plan_writes` truth table over synthetic Artifacts + the `.gnr8/` crate scaffold + the
# naming-override $ref rewrites). These invoke the goextract helper via `go run`, pipe Go through
# `gofmt`, run `go build`/`go test`, and (for `generate_e2e` / `worker_contract`) cargo-compile + run
# the scaffolded worker crate, so the Go + cargo toolchains must be present. The timing-tolerant `watch_smoke` smoke is
# `#[ignore]`d (FS-event flakiness) and is therefore NOT in this blocking line — run it opt-in with
# `cargo test -p gnr8-cli --test watch_smoke -- --ignored`. Mirrors the CI `gates` job (RUST-03 / D-07).
gates:
	cargo test -p gnr8
	cargo test -p gnr8-engine
	cargo test -p gnr8-cli
	cargo test -p gnr8-engine --test snapshot_graph --test snapshot_diagnostics --test snapshot_openapi --test snapshot_sdk --test determinism --test sdk_compile --test pysdk_compile --test tssdk_compile --test sdk_pipeline --test lifecycle --test operation_prose --test cli_emit
	cargo test -p gnr8-engine --test snapshot_nestjs_graph --test snapshot_nestjs_openapi
	cargo test -p gnr8-engine --test sdk_lint

# Restore the `typescript` toolchain for gnr8's OWN test suite (the nestjs snapshot extraction +
# the tssdk_compile typecheck). gnr8 ships NO typescript — in real use `tsextract` borrows the user's
# own `typescript` from the target project (like goextract uses `go`, pyextract uses `python3`). This
# dev install is gitignored and restored on demand. No-op (and the dependent tests skip gracefully)
# when node/npm is absent. `npm ci` is reproducible against the committed package-lock.json; package
# auditing is intentionally excluded because it is unrelated to restoring this dev-only toolchain.
tsextract-deps:
	@command -v npm >/dev/null 2>&1 && (cd tsextract && npm ci --silent --no-audit) || echo "npm absent — TS tests will skip"

pyextract-test:
	python3 -m unittest discover pyextract/tests

tsextract-test: tsextract-deps
	@if command -v node >/dev/null 2>&1; then \
		set -e; cd tsextract; for test in tests/*.test.js; do node "$$test"; done; \
	else \
		echo "node absent — TS sidecar tests skipped"; \
	fi

# Exercise the composite action's exact direct-dependency version resolver against real and
# adversarial lock graphs (including a conflicting transitive gnr8 version), and its Rust toolchain
# resolver against repository pins (so CI compiles `.gnr8` with the same rustc as developer machines).
action-test: action-annotations-test
	bash scripts/test-action-version.sh
	bash scripts/test-action-toolchain.sh
	bash scripts/test-action-changes.sh
	bash scripts/test-action-comment.sh
	bash scripts/test-action-annotations.sh

# Unit coverage for the workflow-command encoder, alongside the release script tests.
action-annotations-test:
	python3 -m unittest discover -s scripts -p 'test_emit_action_annotations.py'

# Keep release publication tied to a dated changelog section with an empty Unreleased section.
release-notes-test:
	python3 -m unittest discover -s scripts -p 'test_release_notes.py'

# Compile + vet the standalone Go Gin fixture module (Pitfall 5 — cargo never builds it).
fixture-build:
	cd fixtures/goalservice && go build ./... && go vet ./...

# Build + vet + test the standalone goextract helper module (cargo never builds it).
# Mirrors the fixture-build gate; the helper is the Go side of the Rust<->Go contract.
goextract-build:
	cd goextract && go build ./... && go vet ./... && go test ./...

# Historical red-by-design target (Phase 1 / v2.0). The six multi-language acceptance snapshots
# (FastAPI/Flask/NestJS graph + OpenAPI) were `#[ignore]`d red-by-design until their extractors
# landed; ALL SIX are GREEN now (pyextract — Phase 2; tsextract — Phase 4 / Plan 04-03) and run in the
# blocking `gates:` set, so nothing remains `#[ignore]`d here. This target is kept as a no-op marker
# of where the honest-red contract used to live; the `-` prefix keeps it non-aborting.
red:
	@echo "no red-by-design acceptance snapshots remain — all six are GREEN in the gates target"

# Cross-language byte-identical determinism gate (XLANG-05). Build the release `gnr8` binary ONCE,
# then for each of the three end-to-end examples (Go / Python / TypeScript) `cd` in, run `gnr8
# generate`, then `gnr8 check` — which DRY-RUNS the same write plan and exits NON-ZERO on any drift
# (crates/gnr8/src/main.rs run_check). `gnr8 check` IS the regen-and-diff, so no bespoke compare
# script is written (CLAUDE.md rule 2 / Don't-Hand-Roll). The committed `examples/*/generated/` bytes
# are thereby asserted to equal a fresh `gnr8 generate` (T-06-04: hand-edited bytes fail the gate).
#
# `gnr8 generate` builds the `.gnr8/` worker with `cargo build` on its first run and runs the
# per-language sidecar
# (`go` / `python3` / `node` + the dev-installed `typescript`), so those toolchains must be on PATH.
# In this sandbox `go` is NOT on the default PATH (it lives under the relocatable install dir), so the
# recipe prepends it; cargo/node/python3 are already on the PATH `make` inherits. The NestJS example
# needs the dev `typescript` restored first (`tsextract-deps`), matching gnr8's own test suite.
GNR8_BIN := target/release/gnr8
GO_BIN := /home/vercel-sandbox/.local/go-install/go/bin
examples-check: tsextract-deps
	cargo build --release -p gnr8-cli
	@set -e; \
	tmp="$$(mktemp -d)"; \
	trap 'rm -rf "$$tmp"' EXIT; \
	mkdir -p "$$tmp/examples"; \
	for dir in examples/*/generated; do mkdir -p "$$tmp/$$dir"; cp -R "$$dir"/. "$$tmp/$$dir"/; done; \
	export GNR8_RESOURCE_DIR="$(CURDIR)"; \
	PATH="$$PATH:$(GO_BIN)" sh -c 'cd examples/bookstore         && "$(CURDIR)/$(GNR8_BIN)" generate --force && "$(CURDIR)/$(GNR8_BIN)" check'; \
	PATH="$$PATH:$(GO_BIN)" sh -c 'cd examples/taskflow          && "$(CURDIR)/$(GNR8_BIN)" generate --force && "$(CURDIR)/$(GNR8_BIN)" check'; \
	PATH="$$PATH:$(GO_BIN)" sh -c 'cd examples/fastapi-bookstore && "$(CURDIR)/$(GNR8_BIN)" generate --force && "$(CURDIR)/$(GNR8_BIN)" check'; \
	PATH="$$PATH:$(GO_BIN)" sh -c 'cd examples/flask-bookstore   && "$(CURDIR)/$(GNR8_BIN)" generate --force && "$(CURDIR)/$(GNR8_BIN)" check'; \
	PATH="$$PATH:$(GO_BIN)" sh -c 'cd examples/nestjs-bookstore  && "$(CURDIR)/$(GNR8_BIN)" generate --force && "$(CURDIR)/$(GNR8_BIN)" check'; \
	for dir in examples/*/generated; do diff -ru "$$tmp/$$dir" "$$dir"; done

# Enforce CLAUDE.md rule 0: one native contract, no foreign annotation/compatibility coupling.
invariants:
	scripts/check-invariants.sh
	python3 scripts/check-ci-budget.py
	$(MAKE) release-notes-test

# Full local gate; CI runs these confidence areas as focused jobs.
check: invariants fmt-check clippy tsextract-deps test fixture-build goextract-build pyextract-test tsextract-test action-test examples-check

all: check
