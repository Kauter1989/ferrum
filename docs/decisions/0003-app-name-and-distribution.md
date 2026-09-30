# ADR 0003 — Application name and distribution

- **Status:** Accepted
- **Date:** 2026-09-30

## Context

The application crate kept its monorepo-era name, `mri-viewer`, and was
started with `cargo run --release -p mri-viewer`. The name did not match
the repository, and users without a Rust toolchain had no way to run the
viewer.

## Decision

- The presentation crate, its binary and its library are named
  `dicom_renderer` (`crates/dicom_renderer`). The other crates keep the
  `mri-` prefix, because they are internal layers.
- The application is the workspace's `default-members` entry, so
  `cargo run --release` starts it from the repository root. Workspace-wide
  commands always pass `--workspace`.
- `cargo install --git https://github.com/Kauter1989/dicom_renderer dicom_renderer`
  and `make install` install the binary.
- Pushing a `v*` tag runs `.github/workflows/release.yml`. It publishes
  prebuilt archives for Linux x86_64, macOS arm64 and Windows x86_64 on the
  GitHub release.

Docker is not offered as a way to run the viewer. It is a GPU desktop
application, and a container would need display-server sockets and GPU
device pass-through, which differ per host. On macOS and Windows, Docker
has no access to the host GPU at all. Prebuilt binaries are simpler on
every platform.

## Consequences

ADR 0001 still refers to the crate as `mri-viewer`, the name it had at the
time. Documentation and tooling use `dicom_renderer`.
