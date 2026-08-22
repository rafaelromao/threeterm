# ThreeTerm

ThreeTerm is a Linux terminal-native parametric CAD product for designing
functional parts for 3D printing. This repository hosts the Rust implementation
of the ThreeTerm MVP. The architecture and product specification are recorded
in issue #58; this README documents the current module map.

## Module map

The Rust workspace has exactly twelve member crates, organised per the closed
OCCT/libslvs architecture decisions (issues #26 and #25). Each member crate
owns its own per-crate `schema_version()` constant under the spec's
`Project Manifest` model.

| Member crate                  | Package name               | Responsibility |
| ----------------------------- | -------------------------- | -------------- |
| `crates/host`                 | `threeterm-host`           | Rust host that owns the Revision Snapshot, versioned command API, lifecycle, and worker process boundaries. |
| `crates/workers/occt`         | `threeterm-occt-worker`    | Rust skeleton crate for the disposable OCCT geometry worker boundary. C++ worker code lives outside the workspace. |
| `crates/workers/slvs`         | `threeterm-slvs-worker`    | Rust skeleton crate for the disposable `libslvs` sketch-solver worker boundary. C++ worker code lives outside the workspace. |
| `crates/tui`                  | `threeterm-tui`            | Keyboard-first TUI adapter for the versioned domain command API. |
| `crates/cli`                  | `threeterm-cli`            | Headless Automation CLI adapter for the versioned domain command API. |
| `crates/mcp`                  | `threeterm-mcp`            | MCP adapter exposing the versioned domain command API as agent tools. |
| `crates/viewport`             | `threeterm-viewport`       | Protocol-Neutral Viewport renderer and projection boundary. |
| `crates/persistence`          | `threeterm-persistence`    | Canonical Transaction Log (NDJSON-encoded) and sealed `.threeterm/` project bundle. |
| `crates/theme`                | `threeterm-theme`          | Embedded palette resolution for the five theme families. |
| `crates/lua-bridge`           | `threeterm-lua-bridge`     | Restricted Lua bridge for keymaps and registered-command automation. |
| `crates/domain`               | `threeterm-domain`         | Canonical ThreeTerm feature graph and domain model. |
| `crates/protocol`             | `threeterm-protocol`       | Versioned newline-framed worker protocol shared by host and disposable workers. |

## Toolchain

The pinned Rust toolchain is recorded in `rust-toolchain.toml` (channel,
components, targets) and mirrored as a single-line string in
`rust-toolchain-channel.txt`. CI and local development install the same
exact toolchain via rustup.

```
channel = "1.97.1"
components = ["rustfmt", "clippy"]
targets = ["x86_64-unknown-linux-gnu"]
```

## Local verification

```sh
# Format check
cargo fmt --all -- --check

# Lint gate (clippy with warnings-as-errors on every target)
cargo clippy --workspace --all-targets -- -D warnings

# Compile every member crate
cargo check --workspace

# Run every member crate's schema-version test
cargo test --workspace
```

The local acceptance verifier `.github/scripts/acceptance.sh` runs the four
checks above and additionally asserts the toolchain pin and the exact
twelve-member workspace contract.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and every pull
request targeting `main`. The workflow:

1. checks out the workspace,
2. installs Podman for the runner's default user,
3. asserts `podman info` reports rootless operation,
4. runs the canonical CI script `.github/scripts/ci.sh` inside
   `docker.io/archlinux:latest` via rootless Podman.

The CI script installs rustup and the pinned toolchain inside the
archlinux container, then runs `cargo check`, `cargo fmt --check`,
`cargo clippy -D warnings`, and `cargo test`.

<a href="https://github.com/rafaelromao/sandman">
  <img src="https://raw.githubusercontent.com/rafaelromao/sandman/main/assets/badge-built-with-sandman.svg" alt="Built with Sandman" width="154" />
</a>
