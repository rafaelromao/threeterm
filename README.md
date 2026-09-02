# ThreeTerm

ThreeTerm is a Linux terminal-native parametric CAD product for designing
functional parts for 3D printing. This repository hosts the Rust implementation
of the ThreeTerm MVP. The architecture and product specification are recorded
in issue #58; this README documents the current module map.

## Module map

The Rust workspace has exactly thirteen member crates, organised per the closed
OCCT/libslvs architecture decisions (issues #26 and #25). Each member crate
owns its own per-crate `schema_version()` constant under the spec's
`Project Manifest` model.

| Member crate                  | Package name               | Responsibility |
| ----------------------------- | -------------------------- | -------------- |
| `crates/host`                 | `threeterm-host`           | Rust host that owns the Revision Snapshot, versioned command API, lifecycle, and worker process boundaries. |
| `crates/workers/occt`         | `threeterm-occt-worker`    | Rust skeleton crate for the disposable OCCT geometry worker boundary. C++ worker code lives outside the workspace. |
| `crates/workers/slvs`         | `threeterm-slvs-worker`    | Rust skeleton crate for the disposable `libslvs` sketch-solver worker boundary. C++ worker code lives outside the workspace. |
| `crates/tui`                  | `threeterm-tui`            | Production direct-Ghostty Interactive Modeling executable and keyboard-first adapter for the versioned domain command API. |
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

# Run the fast test suite used by pull-request CI
bash .github/scripts/test-suite.sh fast

# Run only the opt-in slow tests
bash .github/scripts/test-suite.sh slow

# Run every test, including native-worker and slow E2E coverage. This is the
# same expensive immutable-worker setup used by the manual E2E workflow.
bash .github/scripts/e2e.sh
```

The local acceptance verifier `.github/scripts/acceptance.sh` runs the four
checks above and additionally asserts the toolchain pin and the exact
thirteen-member workspace contract.

## Test suites

`#[ignore = "slow: ..."]` identifies a long-running test. Pull-request CI
runs the fast suite with native worker construction disabled. The manually
triggered native E2E workflow runs the complete suite, including ignored tests,
against the immutable OCCT and libslvs workers. The slow suite remains a
smaller local opt-in that runs only ignored tests.

The fast suite retains representative coverage for each product boundary:

| Behavior | Fast coverage |
| --- | --- |
| Geometric Kernel operations | Small real-OCCT operation tests in `crates/workers/occt/tests/worker_integration.rs` and CLI operation tests |
| Headless Automation | CLI command, error, save/load, export, and schema tests |
| Interactive Modeling | TUI interaction, routing, cleanup, and viewport tests |
| MCP adapter | MCP command and component-instance tests |
| Canonical Transaction Log and recovery | persistence, host, migration, and historical-recovery tests |
| Protocol and worker supervision | protocol framing, registry, worker, and supervisor tests |
| Sketches, fit relationships, and viewport projection | sketch-solve, host fit-dimension, persistence fit-dimension, and viewport tests |
| Rehearsal contract | registered schema, CLI argument, and fast timing-comparison tests |

The slow suite repeats complete release candidates, cross-worker workflows, and
native adversarial or exhaustive geometry checks. It adds confidence in their
composition without delaying every pull request.

## Continuous integration

`.github/workflows/ci.yml` runs on every push to `main` and every pull
request targeting `main`, with a ten-minute job timeout. The workflow:

1. checks out the workspace,
2. restores Cargo dependencies and build artifacts,
3. runs `.github/scripts/ci.sh` with native worker construction disabled.

The CI script installs the pinned Rust toolchain when necessary, then runs
`cargo check`, `cargo fmt --check`, `cargo clippy -D warnings`, and the fast
test suite.

`.github/workflows/e2e.yml` is manually triggered. It retains the rootless
Arch container and immutable source-built OCCT/libslvs workers, then runs the
complete native E2E suite and release contracts without blocking pull requests.

<a href="https://github.com/rafaelromao/sandman">
  <img src="https://raw.githubusercontent.com/rafaelromao/sandman/main/assets/badge-built-with-sandman.svg" alt="Built with Sandman" width="154" />
</a>
