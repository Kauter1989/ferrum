# Build shortcuts.

.PHONY: run build install test test-gpu lint bench snapshot showcase

run:        ## run the desktop viewer (pass ARGS=path/to/dicom)
	cargo run --release -- $(ARGS)

build:      ## release build of the viewer
	cargo build --release

install:    ## install the dicom_renderer binary into ~/.cargo/bin
	cargo install --path crates/dicom_renderer --locked

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

showcase:   ## README screenshots from a real dataset (ARGS="<volume> <out_dir>")
	cargo run --release --example showcase -- $(ARGS)
