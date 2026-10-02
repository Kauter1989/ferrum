# Build shortcuts.

.PHONY: run build install test test-gpu lint licenses coverage coverage-ci complexity bench snapshot showcase

run:        ## run the desktop viewer (pass ARGS=path/to/dicom)
	cargo run --release -- $(ARGS)

build:      ## release build of the viewer
	cargo build --release

install:    ## install the ferrum binary into ~/.cargo/bin
	cargo install --path crates/ferrum --locked

test:       ## full test suite (GPU tests skip without an adapter)
	cargo test --workspace

test-gpu:   ## full test suite, failing if no GPU/lavapipe adapter exists
	FERRUM_REQUIRE_GPU=1 cargo test --workspace

lint:       ## rustfmt + clippy with warnings as errors
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

licenses:   ## dependency licences must fit MIT OR Apache-2.0 (needs cargo-deny)
	cargo deny check licenses

# Code that tests cannot reach meaningfully: binary entry point, examples, benches.
COVERAGE_IGNORE := (main\.rs|/examples/|/benches/)
# Line-coverage floor enforced in CI (baseline 88.5 % at the time it was set).
COVERAGE_FLOOR := 87

coverage:   ## test coverage, HTML report in target/llvm-cov/html (needs cargo-llvm-cov)
	cargo llvm-cov --workspace --ignore-filename-regex '$(COVERAGE_IGNORE)' --html
	cargo llvm-cov report --ignore-filename-regex '$(COVERAGE_IGNORE)' --summary-only

coverage-ci: ## coverage for CI: lcov + HTML + summary, fails below COVERAGE_FLOOR
	cargo llvm-cov clean --workspace
	cargo llvm-cov --workspace --no-report
	cargo llvm-cov report --ignore-filename-regex '$(COVERAGE_IGNORE)' --lcov --output-path lcov.info
	cargo llvm-cov report --ignore-filename-regex '$(COVERAGE_IGNORE)' --html
	cargo llvm-cov report --ignore-filename-regex '$(COVERAGE_IGNORE)' --summary-only --fail-under-lines $(COVERAGE_FLOOR)

complexity: ## complexity budget only (cognitive complexity, function length, nesting)
	cargo clippy --workspace --all-targets -- -D clippy::cognitive_complexity -D clippy::too_many_lines -D clippy::excessive_nesting

bench:      ## criterion benchmarks
	cargo bench --workspace

snapshot:   ## headless PNG renders of every mode (ARGS="<input> <out_dir>")
	cargo run --release -p ferrum-render --features gpu --example snapshot -- $(ARGS)

showcase:   ## README screenshots from a real dataset (ARGS="<volume> <out_dir>")
	cargo run --release --example showcase -- $(ARGS)
