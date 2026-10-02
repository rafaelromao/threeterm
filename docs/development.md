# Developing ThreeTerm

ThreeTerm is a Linux terminal-native parametric CAD product for designing
functional parts for 3D printing. This repository hosts the Rust implementation
of the ThreeTerm MVP. The implementation-facing contract is recorded in
[`docs/mvp-implementation-specification.md`](mvp-implementation-specification.md),
with issue #58 retained as planning provenance. This guide documents the current
module map and verification commands.

For installation and the hobbyist modeling walkthrough, start with the
[user README](../README.md). The static landing page and deployment instructions
are described in [website.md](website.md).

## Module map

The Rust workspace has exactly thirteen member crates, organised per the closed
OCCT/libslvs architecture decisions (issues #26 and #25). Each member crate
owns its own per-crate `schema_version()` constant under the spec's
`Project Manifest` model.

| Member crate                  | Package name               | Responsibility |
| ----------------------------- | -------------------------- | -------------- |
| `crates/host`                 | `threeterm-host`           | Rust host that owns the Revision Snapshot, versioned command API, lifecycle, and worker process boundaries. |
| `crates/workers/occt`         | `threeterm-occt-worker`    | Rust adapter that locates, authenticates, and supervises the disposable OCCT geometry worker. Native C++ code is in this crate's `src-cpp/` directory. |
| `crates/workers/slvs`         | `threeterm-slvs-worker`    | Rust adapter for the disposable `libslvs` sketch solver, including typed sketch requests and solver diagnostics. Native C++ code is in this crate's `src-cpp/` directory. |
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

# Run the native E2E suite, including ignored native tests
bash .github/scripts/e2e.sh

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

## Resource-bounded local native checks

The CI, native E2E, and three-journey scripts default to one Cargo build job.
For native checks on a desktop, run one test command at a time and place its
temporary projects on disk rather than in a memory-backed `/tmp`. On a systemd
Linux desktop, a transient user scope also bounds the whole test process tree:

```sh
mkdir -p target/release-e2e-tmp
systemd-run --user --scope --quiet \
  -p CPUQuota=100% -p MemoryHigh=1G -p MemoryMax=2G \
  -p MemorySwapMax=0 -p TasksMax=64 -p IOWeight=10 \
  nice -n 15 env CARGO_BUILD_JOBS=1 TMPDIR="$PWD/target/release-e2e-tmp" \
  cargo test -p threeterm-host --test domain_command_executor \
  --jobs 1 -- --test-threads=1
```

Configure the pinned native worker prefixes before running native tests, as for
the ordinary local verification commands. The scope limits CPU usage to one
core, caps memory at 2 GiB, and permits no additional swap. Reaching the memory
limit fails the test process inside the scope. These limits are transient and
do not change desktop settings.

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

Release PR verification dispatches `.github/workflows/three-journey.yml`. The
API and MCP journeys run in the pinned rootless Arch container, while the TUI journey runs
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

For a narrated run with the TUI visible on your desktop, launch the showcase
from a Wayland desktop terminal:

```sh
bash .github/scripts/three-journey-showcase.sh
```

The API and MCP test output streams in the launch terminal. During the TUI
journey, a fullscreen nested Weston/Ghostty window shows the scripted
keyboard-first modeling flow, including previews, commits, navigation, and STL
export. The same evidence catalog and per-surface logs are retained under
`target/three-journey-showcase` by default; set
`THREETERM_THREE_JOURNEY_ROOT` to choose another location.

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

## Commit and release workflow

Pull request titles use Conventional Commits, including an optional scope, for
example `feat(host): add a modeling command` or
`fix(persistence): preserve project identity`. The semantic title check accepts
the standard release types used by this repository. Use a scope naming the
affected crate, product surface, or automation area (`host`, `mcp`, `tui`,
`persistence`, `ci`, `release`); omit it for cross-cutting changes. Release
Please groups the merged commit history into a single release PR and carries
scopes into the generated changelog. The release manifest starts at `0.1.0`.
The `version.txt` marker, workspace package version, and local package entries
in `Cargo.lock` are updated together in the generated release PR. Before
publishing a merged release PR, `.github/scripts/release.sh verify` checks the
signed release-namespace gate; an unsigned or stale gate stops tag and GitHub
Release creation.

### Release E2E orchestration

After creating or updating the pending release PR, the Release Please workflow
dispatches `native-e2e` and `three-journey-gate` against that PR's head commit.
The `Release E2E gate` commit status passes only when both workflows pass.

The native workflow sets `THREETERM_ACCEPTANCE_SCOPE=native`. Its acceptance
catalog records `scope: "native"` and `deferred_gates: ["coverage.all-surfaces"]`,
and requires complete evidence for all twenty native gates, including
`release.namespace`. Graphical coverage is required by the separate
three-journey aggregate, which also compares the three retained STL artifacts.
The native test selectors exclude that cross-surface aggregate because its TUI
evidence is produced by the qualified graphical runner. Running `acceptance.sh`
locally defaults to `scope: "full"` and executes all twenty-one gates.

To complete a release run:

1. Register an online Linux x64 self-hosted Actions runner with the
   `threeterm-graphical` label under **Settings → Actions → Runners**. Install
   the [Official Interactive Environment prerequisites](#official-interactive-environment-verification)
   and set `THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT` in the runner's environment
   to the qualified version-contract file. Keep the runner available for the
   TUI job; a queued job with no matching runner cannot publish passing evidence.
2. Complete and commit the current owner-signed
   [namespace release gate](release/trademark-and-namespace-gate.md).
   `bash .github/scripts/release.sh verify` must pass before the release
   acceptance catalog can pass.
3. Merge the E2E fixes into `main`, then let Release Please update the release
   PR and dispatch checks for its new head commit. Re-running an old workflow
   run uses its original checkout and does not pick up these fixes.

### Run Release Please PR checks without workflow approval

GitHub puts `pull_request` workflows triggered by a PR created or updated with
the built-in `GITHUB_TOKEN` into an **approval-required** state. This applies to
the fast CI and semantic-title checks. Explicit `workflow_dispatch` events,
including this repository's release E2E dispatches, run automatically with
`GITHUB_TOKEN`.

Use a fine-grained personal access token for the Release Please action to make
its PR checks run automatically:

1. Create a token scoped to `rafaelromao/threeterm`, with **Contents: read and
   write** and **Pull requests: read and write**. Grant **Workflows: read and
   write** if Release Please needs to update workflow files on its branch.
2. Store it as the repository Actions secret `RELEASE_PLEASE_TOKEN` under
   **Settings → Secrets and variables → Actions**.
3. The Release Please step in `.github/workflows/release-please.yml` already
   uses this secret when present and falls back to the built-in token otherwise:

   ```yaml
   - name: Create or update the release pull request
     uses: googleapis/release-please-action@v4
     with:
       token: ${{ secrets.RELEASE_PLEASE_TOKEN || github.token }}
       config-file: release-please-config.json
       manifest-file: .release-please-manifest.json
   ```

4. Keep **Allow GitHub Actions to create and approve pull requests** enabled
   under **Settings → Actions → General → Workflow permissions**. The workflow
   declares its own write permissions, so the default token permissions can
   remain read-only.

An installed GitHub App can also provide the Release Please token: generate its
short-lived installation token with `actions/create-github-app-token` and pass
that step's `outputs.token` to Release Please. Both App and PAT credentials
allow automatic PR workflows without the built-in-token approval prompt.

Using an App/PAT also enables normal `push` events. The slow suites use only
`workflow_dispatch`, so adding the secret does not create duplicate E2E runs.
The dispatcher waits for those runs and publishes the combined release status.
Its `GH_TOKEN` continues using `GITHUB_TOKEN` with `actions: write` and
`statuses: write`.

Changing fork-contributor approval settings does not remove the approval
requirement for PRs created with `GITHUB_TOKEN`.
See GitHub's [workflow-triggering documentation](https://docs.github.com/en/actions/how-tos/writing-workflows/choosing-when-your-workflow-runs/triggering-a-workflow#triggering-a-workflow-from-a-workflow).

## Test suites

`#[ignore = "slow: ..."]` identifies a long-running test. Pull-request CI
runs the fast suite with native worker construction disabled. The manually
triggered native E2E workflow runs the native suite, including ignored tests,
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

`.github/workflows/e2e.yml` retains the rootless Arch container and immutable
source-built OCCT/libslvs workers. Its native acceptance catalog and complete
ignored E2E suite run only for a Release Please pull request carrying the
`autorelease: pending` label, or when manually dispatched. The full suite is
run through `.github/scripts/e2e.sh`; ordinary feature PRs run only fast CI.
Release Please dispatches both native E2E workflows at the release branch after
updating the PR, because its `GITHUB_TOKEN` cannot trigger follow-up PR events.
It waits for both runs and publishes a `Release E2E gate` status check on the
release PR commit, so the slow results appear with that PR's checks.
That dispatch expects the acceptance catalog to pass, preserving the signed
release-namespace gate; the manual run below expects the current unsigned-gate
failure unless `passed` is selected after the gate is signed.
Maintainers can also run the workflows manually:

```sh
gh workflow run e2e.yml --ref main -f expected_catalog_result=failed
gh workflow run three-journey.yml --ref main
```

`.github/workflows/release-please.yml` opens or updates the release PR after
commits reach `main`. `.github/workflows/semantic-pull-request.yml` validates
PR titles so the release notes have Conventional Commit types and scopes to
interpret.

<a href="https://github.com/rafaelromao/sandman">
  <img src="https://raw.githubusercontent.com/rafaelromao/sandman/main/assets/badge-built-with-sandman.svg" alt="Built with Sandman" width="154" />
</a>
