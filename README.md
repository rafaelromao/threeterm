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
| `crates/rehearsal`            | `rehearsal`                | Production L-bracket rehearsal and evidence catalog workflow. |

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

# Run the commit-bound production conformance catalog. The command runs every
# required workflow, replay, registry, worker, licensing, release,
# documentation, and performance gate. It always writes
# `target/acceptance-catalog.json`; an unavailable worker or unsigned release
# gate is recorded as failed and returns a non-zero status.
bash .github/scripts/acceptance.sh

# Check the three-journey producer/aggregate shell contract without native
# workers or a graphical runner.
bash tests/three-journey-gate.sh
```

## Official Interactive Environment Verification

The real Ghostty launch is intentionally separate from Headless Automation and
the native Geometric Kernel worker tier. The Official Interactive Environment
must provide Weston with its headless backend, Ghostty `1.3.1-arch2`, `wtype`, `ydotool`, `wlr-randr`,
`grim`, Tesseract, ImageMagick, `jq`, Coreutils, and util-linux. The exact
`--version` output tokens for those tools are recorded in
`.github/graphical-toolchain.env` by the qualified runner environment.

The named test creates a fresh L-bracket Project Generation with the real OCCT
worker, launches Interactive Modeling as Ghostty's child, and retains evidence
under the temporary project root:

```sh
cargo test -p threeterm-tui --test graphical_launch \
  production_tui_ghostty_session -- --ignored --exact --test-threads=1
```

The runner uses `LC_ALL=C.UTF-8`, `LANG=C.UTF-8`, an 800x600 compositor, an
80x24 terminal, and a top-left 800x480 viewport crop. It fails closed when the
version contract, compositor, input, screenshot, OCR, positive capability
probe, visible readiness, or cleanup evidence is missing. It never sets
`TERM` or `TERM_PROGRAM`; those values must come from real Ghostty.

The runner contract can be checked without graphical dependencies:

```sh
bash tests/graphical-runner-contract.sh
```

Each graphical run retains `manifest.json`, PTY input/output logs, TUI and
compositor diagnostics, startup/orbit/cleanup screenshots, and SHA-256 artifact
records. The manifest itself is not included in its own hash list.

The manual native E2E workflow remains available for focused test-tier runs.
The acceptance catalog is the production closure command and records the exact
source commit, schema and worker identities, gate outcomes, and artifact
checksums.

### Three-surface geometric equivalence

The three all-tool journey tests can publish one Journey Evidence Report per
Producer Surface for the geometric equivalence gate. The aggregate is intentionally ignored by the
ordinary workspace suite because it requires Headless Automation through both
API and MCP, plus Interactive Modeling in the Official Interactive Environment.
Run the API and MCP journeys with the native worker, run the TUI journey in the
Official Interactive Environment, then run the one named aggregate test:

```sh
export THREETERM_JOURNEY_EVIDENCE_ROOT="$PWD/target/journey-equivalence"
export THREETERM_JOURNEY_RUN_ID="$(git rev-parse HEAD)-$(date +%s)"

THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
  cargo test -p threeterm-host --test bracket_base_qualification \
  e2e_stl_api_all_tools_l_bracket --jobs 1 -- \
  --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
  cargo test -p threeterm-mcp --test production_lifecycle \
  e2e_stl_mcp_all_tools_l_bracket --jobs 1 -- \
  --include-ignored --exact --test-threads=1
# Run this command in the Official Interactive Environment.
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
  cargo test -p threeterm-tui --test graphical_launch \
  production_tui_all_tools_stl_journey --jobs 1 -- \
  --include-ignored --exact --test-threads=1
cargo test -p threeterm-host --test bracket_equivalence \
  e2e_stl_three_surface_geometric_equivalence --jobs 1 -- \
  --include-ignored --exact --test-threads=1
```

The aggregate independently verifies each Producer Surface's retained STL against the frozen
recipe before comparing dimensions, volume, topology, voids, landmarks, and
surface samples. It writes `geometric-equivalence.json` on pass or failure;
all journey and aggregate reports are stored under the run ID within the
configured evidence root;
raw STL bytes, facet order, generated identities, timestamps, paths, and
transaction IDs are not equivalence keys.

### Three-journey CI gate

Pull-request CI runs `.github/workflows/three-journey.yml`. The API and MCP
journeys run in the pinned rootless Arch container, while the TUI journey runs
only on a self-hosted runner labelled `threeterm-graphical` with
`THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT` configured. The jobs share the run ID,
recipe binding, native-worker bundle, and retained evidence before the aggregate
job writes `target/three-journey-gate/catalog.json`.

Run the same orchestration locally when the native workers and qualified
graphical environment are available:

```sh
THREETERM_JOURNEY_RUN_ID="$(git rev-parse HEAD)-$(date +%s)" \
  bash .github/scripts/three-journey-gate.sh
```

Each producer retains stdout, stderr, attempt metadata, journey reports,
coverage reports, native-worker identities, and geometric comparison output
under `target/three-journey-gate`. The timeout defaults to 900 seconds per
journey and can be adjusted with
`THREETERM_THREE_JOURNEY_TIMEOUT_SECONDS`; process-group cleanup grace is
controlled by `THREETERM_THREE_JOURNEY_KILL_GRACE_SECONDS`. Missing jobs are
recorded as `unrun` and prerequisite failures as `prerequisite_skipped`; neither
can produce a passing catalog.

## Compatibility contract

The interactive MVP supports only a direct local `xterm-ghostty/1.3.1-arch2`
attachment with the positively probed Terminal Capability Vector described as
the Official Interactive Environment. `threeterm-tui` owns that interactive surface;
`threeterm` and `threeterm-mcp` are Headless Automation adapters. The
CLI, MCP, and TUI all consume the same versioned domain command registry and Project
Manifest contract, but CLI and MCP do not provide a graphical viewport or
replace the direct-Ghostty capability gate.

The native conformance workflow uses the pinned rootless Arch image declared in
`.github/workflows/e2e.yml`. An unsigned release runbook is expected to block
public release; the acceptance catalog records that block instead of treating
it as a passing release gate.

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
