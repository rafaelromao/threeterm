# ThreeTerm MVP Implementation Specification

Status: normative repository contract for the implementation described by
[issue #58](https://github.com/rafaelromao/threeterm/issues/58).

The issue remains the planning provenance for this specification. This checked-in
document is the implementation-facing contract: an agent should be able to use it,
the [domain glossary](../CONTEXT.md), and the source tree without reopening the
Wayfinder research history. Research and rehearsal files linked below explain why
the decisions were made; they do not weaken the rules in this document.

## Authority and Scope

ThreeTerm is a Linux terminal-native parametric CAD tool for functional 3D-print
parts. The Rust host owns canonical document state and the versioned command
surface. Geometry and sketch solving are delegated to version-pinned disposable
workers. The MVP is deliberately narrow:

- Interactive Modeling is supported only in a direct local Ghostty attachment with
  the positively probed Official Interactive Environment.
- Headless Automation is available through the CLI and MCP adapters without a
  viewport, pointer, focus, or terminal capability dependency.
- Lua is limited to local keymaps and registered-command automation. Projects do
  not contain or execute scripts.
- The only interactive graphics path is direct Ghostty Kitty RGB/zlib frames. A
  missing or ambiguous capability probe refuses Interactive Modeling instead of
  selecting a text, cell, ANSI, or alternate-renderer fallback.
- Production behavior belongs in the existing workspace crates. This document
  indexes and constrains that behavior; it does not introduce another CAD feature.

The closed Wayfinder map is [issue #1](https://github.com/rafaelromao/threeterm/issues/1).
The repository glossary is authoritative for terms such as `Project Generation`,
`Revision Snapshot`, `Derived Result`, `Stale Last-Valid Geometry`, `Command Draft`,
`Command Preview`, `Transient Interaction State`, and `Gesture Acknowledgement`.

## Domain and Persistence

The canonical feature graph and its deterministic command inputs are the only
authoritative document state. Topology indexes, worker handles, file paths, and
cache entries are never persistence identifiers. ThreeTerm semantic feature IDs
and provenance context are the stable identity carried through edits and workers.

### Revision contract

- A `Revision Snapshot` is immutable host-owned canonical state at one revision.
- A `Project Generation` is one complete sealed on-disk save. Its identity is
  externally inspectable through the generation digest, revision identity and
  hashes, and Canonical Transaction Log position.
- Every accepted operation is one versioned atomic command transaction containing
  validated intent, semantic events, affected IDs, and deterministic inputs.
- A `Derived Result` is non-authoritative until the host validates its request ID,
  source revision, worker fingerprint, artifact kind, byte count, and SHA-256 and
  atomically promotes it for the matching snapshot.
- A failed historical branch stops at its first failure. Retained output is
  `Stale Last-Valid Geometry`: explicitly inspectable, never current, and never
  eligible for validation or export.

### Bundle contract

The inspectable `.threeterm/` bundle contains these logical roles:

```text
project/
  manifest.json
  transactions.log
  checkpoints/       # optional, discardable
  cache/             # optional, non-authoritative
  artifacts/         # optional, non-authoritative
project.previous-generation/
  manifest.json
  transactions.log
```

`manifest.json` is canonical JSON and sealed. `transactions.log` is append-only
NDJSON and sealed per transaction and as a log. The current generation and the
immediately preceding valid generation are the atomic recovery pair. Checkpoints,
cache entries, and worker artifacts may be discarded and rebuilt; they never outrank
the manifest and log.

Load and save behavior is fail-closed:

- Verify the manifest seal and every transaction hash on every load.
- Select only a complete, authenticated current or previous generation. Never
  combine files from two generations.
- Apply a two-epoch forward-only migration policy: read one prior schema epoch
  through a deterministic migration after preserving a sealed pre-migration
  backup. Writers emit the exact current epoch; downgrade is unsupported.
- Reject unknown fields, feature kinds, command versions, worker identities, or
  malformed canonical records with a structured diagnostic.
- Discard an accelerator whose revision, worker fingerprint, cache key, or digest
  does not match canonical state.
- Publish a new generation through staging and atomic promotion. Interrupted
  publication must leave the prior valid generation loadable.

## Worker and Command Boundaries

The Rust host owns lifecycle, document state, command validation, persistence, and
worker process boundaries. C++ implementation details never cross the protocol.

- One disposable version-pinned C++ OCCT worker handles each geometry request.
- One disposable version-pinned C++ SolveSpace `libslvs` worker handles each
  sketch solve request under its declared GPLv3 or commercial licensing path.
- Startup negotiates the versioned newline-framed worker protocol. Workers expose
  structured messages and staged artifacts, never native pointers, handles, or
  mutable object identity.
- The host requests cooperative cancellation first. After the operation-class
  grace period, it terminates an unresponsive disposable worker. Terminated output
  is discarded and the host records request ID, stage, elapsed time, last progress,
  exit signal, and reproducible inputs. The last committed revision remains intact.
- Staged binary artifacts are accepted only after path, size, digest, schema,
  revision, and worker-fingerprint validation. JSON carries control data, not
  unbounded geometry payloads.

One versioned domain command registry powers TUI, CLI, and MCP. Framing differs;
domain semantics do not. The registry includes project lifecycle, sketch solving,
extrude, Booleans, fillet, chamfer, hole, revolve, mirror, linear pattern, circular
pattern, shell, draft, loft, components, timeline, validation, export, and rehearsal
commands. The L-bracket rehearsal command is `threeterm.command.bracket/1`.

Every caller follows the same command lifecycle:

1. Bind explicit inputs to the caller's source revision.
2. Evaluate a cancellable read-only `Command Preview` using the same OCCT worker
   class as commit. Preview creates no transaction and no canonical mutation.
3. Revalidate the unchanged `Command Draft` against the current revision.
4. Atomically commit one validated transaction or return diagnostics without a
   partial mutation. A stale revision is a structured conflict.

## Validation Journeys

These seven journeys are launch requirements. Each must be operable through TUI,
CLI, and MCP. CLI and MCP are not alternate domain implementations: they must
produce the same structured result, diagnostics, revision fencing, transaction
identity, and recovery outcome as the shared command boundary used by TUI.

### 1. L-shaped bracket

- Start with a fresh Project Generation and create the canonical L-bracket profile.
- Solve its sketch, create the additive body, place the two through holes, validate
  the current solid, and export STL, 3MF, and STEP.
- Observe a sealed manifest/log, revision-bound derived artifacts, export metadata,
  and the canonical rehearsal evidence catalog.
- A non-positive dimension or wall-breaking hole is fatal, does not create a
  transaction, and preserves the last valid revision. Warning-only quality issues
  require an explicit export override.

### 2. Box with lid

- Create a box body and a separate lid body through the shared command registry.
- Observe both bodies in the canonical feature graph after save and reload, with
  independently addressable semantic IDs and no topology-index persistence.
- An invalid Boolean, shell, or export input returns a structured failure and leaves
  both the prior graph and the prior valid generation unchanged.

### 3. Reusable component

- Define a component from existing selected features, create a transformed linked
  instance, and create an independent copy.
- Edit the source definition and observe linked reuse update while the independent
  copy remains divergent and addressable by its own semantic IDs.
- Lost, ambiguous, or incompatible references are explicit outcomes; no edit may
  silently select a different feature.

### 4. Historical edit

- Commit a valid sequence, edit an earlier command, and recompute only the affected
  transitive branches from the new Revision Snapshot.
- On the first failed branch, observe the branch stop, explicit Stale Last-Valid
  Geometry, and correction, suppression, or named-revision recovery choices.
- Unaffected work remains available. Stale geometry cannot become current or pass
  export validation.

### 5. Object-specific timeline

- Use one active linear timeline for undo and redo, then filter the history view by
  feature or component identity.
- Diverging after undo preserves the abandoned future as a recoverable named
  revision; it does not create an in-app merge graph.
- Timeline responses expose current revision, affected IDs, failure state, and
  recovery status consistently through all three adapters.

### 6. Keyboard-first modeling

- Start only after direct Ghostty capability probes pass. Activate the modeless
  command palette from the keyboard and collect exactly one modal Command Draft.
- Show a visible Gesture Acknowledgement for selection, focus, drag, resize,
  cancellation, readiness, and recovery. Every transient state also has a
  non-color marker.
- Preview is transient; only an explicit revalidated commit changes the document.
  Rapid camera changes coalesce to the newest pending viewport state.

### 7. Recovery from invalid edit

- Preview and attempt an invalid edit against a known valid revision.
- Observe fatal diagnostics, no transaction, no revision advance, and unchanged
  canonical files. The last valid geometry remains the only exportable state.
- If publication is interrupted, load the current or previous complete generation;
  never accept a partial generation or silently repair unknown canonical data.

## Interactive Presentation and Caching

The Protocol-Neutral Viewport renders the latest validated Layer 1 Derived Result
under the latest Layer 2 display key. Layer 1 is revision-bound and OCCT-worker-
issued. Layer 2 is host-only and keyed by revision, worker fingerprint, Layer 1
reference, viewport dimensions, camera frustum band, quality level, selection
fingerprint, and preview scope.

There is one attachment-scoped active Kitty image, at most one in-flight frame, and
one newest pending state. Obsolete camera states are dropped rather than rendered
in order. Ghostty acknowledgements, reset, resize, focus loss, selection changes,
close, and cancellation clean up transient state while preserving the last committed
revision. Missing or ambiguous capability probes return a structured diagnostic and
route the caller to Headless Automation.

The input model keeps `Lifecycle`, `Focus + Capture`, `Selection`, `Interaction
mode`, `Command phase`, and active linear `History` axes orthogonal. Hover, middle
button, pixel precision, and key release are never correctness-critical.

## Themes, Lua, and Release Gates

The embedded theme families are Catppuccin dark, TokyoNight dark, EverGreen dark,
GruvBox dark, and Sandman light. Resolution order is `--palette`,
`THREETERM_PALETTE`, then configuration. Invalid or unsupported palettes fail closed
with structured diagnostics; there are no silent substitutions. Twelve viewport
semantic RGB tokens and seven TUI overlay tokens are contrast-checked, and every
transient state carries a non-color marker.

Lua may bind keys and invoke registered commands only. It cannot introduce feature
logic, dynamic plugins, project-embedded scripts, or persistent script state.

No performance claim is admitted from a single spike. The six gates are rehearsal
spike, representative hardware, representative project scale, declared statistical
method, fixture-versus-product limits, and two-release regression. Measured bands
remain evidence until all six gates promote them to a target. Public release is
blocked until the signed trademark and namespace gate passes through
[`docs/release/trademark-and-namespace-gate.md`](release/trademark-and-namespace-gate.md).

## Module and Work Map

| Contract area | Workspace owner | Boundary | Status |
|---|---|---|---|
| Canonical graph and components | `crates/domain` | stable semantic IDs, sketches, references, history model | Implemented; crate tests cover the boundary |
| Persistence and recovery | `crates/persistence` | sealed manifest, NDJSON log, migration, generations | Implemented; crate tests cover the boundary |
| Command registry and protocol | `crates/protocol` | versioned schemas, framing, validation, supervision | Implemented; crate tests cover the boundary |
| Host execution | `crates/host` | snapshots, workers, derived-result promotion, export | Implemented; host tests cover the boundary |
| OCCT worker boundary | `crates/workers/occt` | disposable C++ geometry and artifact protocol | Implemented; native tests remain environment-gated |
| libslvs worker boundary | `crates/workers/slvs` | disposable C++ sketch solving and diagnostics | Implemented; native tests remain environment-gated |
| Direct-Ghostty TUI | `crates/tui` | keyboard state, command palette, capability routing | Implemented; graphical tests remain environment-gated |
| Headless CLI | `crates/cli` | machine-readable adapter over the shared command API | Implemented; adapter tests cover the boundary |
| MCP | `crates/mcp` | JSON-RPC/MCP adapter over the shared command API | Implemented; adapter tests cover the boundary |
| Viewport | `crates/viewport` | projection, Kitty frames, cache and coalescing | Implemented; renderer tests cover the boundary |
| Themes | `crates/theme` | embedded palette resolution and diagnostics | Implemented; palette tests cover the boundary |
| Lua boundary | `crates/lua-bridge` | restricted keymap and registered-command bridge | Implemented; bridge tests cover the boundary |
| Rehearsal | `crates/rehearsal` | L-bracket evidence and release-candidate comparison | Implemented; rehearsal tests cover the boundary |

Implementation order follows the closed child-spec bands: f1 foundation, f2
interactive core, f3 feature depth, then f4 rehearsal and polish. Within an area,
land `v1` before `v2`, `v3`, and `v4`. The parent issue is a specification index;
production changes belong to its child slices and their tests.

## Verification Contract

The documentation contract is checked without native workers or a graphical
environment:

```sh
bash tests/mvp-specification-contract.sh
```

The normal fast regression tier is:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
bash .github/scripts/test-suite.sh fast
```

Native worker, slow, complete E2E, and Official Interactive Environment commands
remain separate gates. See [README.md](../README.md),
[`docs/research/rehearsal-evidence/`](research/rehearsal-evidence/), and the release
runbooks for their required environments and evidence.
