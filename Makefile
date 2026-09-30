# Build shortcuts.

.PHONY: run build test test-gpu lint bench snapshot

run:        ## run the desktop viewer (pass ARGS=path/to/dicom)
	cargo run --release -p mri-viewer -- $(ARGS)

build:      ## release build of the viewer
	cargo build --release -p mri-viewer

test:       ## full test suite (GPU tests skip without an adapter)
	cargo test --workspace

test-gpu:   ## full test suite, failing if no GPU/lavapipe adapter exists
	MRI_REQUIRE_GPU=1 cargo test --workspace

lint:       ## rustfmt + clippy with warnings as errors
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

bench:      ## criterion benchmarks
	cargo bench --workspace

snapshot:   ## headless PNG renders of every mode (ARGS="<input> <out_dir>")
	cargo run --release -p mri-render --features gpu --example snapshot -- $(ARGS)
