# Fast native-contract coverage inventory

Scope: issue #521. Fast PR CI must prove adapter, Revision Snapshot,
Canonical Transaction Log, and Derived Result binding without the Geometric
Kernel. Native E2E remains the owner of real geometry, real worker
process/protocol evidence, solver output, and supply-chain/release gates.

## Fast-contract covered

- Boolean fuse/cut/common CLI command spelling and distinct worker
  `command_id` values (`crates/cli/tests/fast_native_contracts.rs`).
- Boolean operand binding: request `base_path`/`tool_path` resolve to the
  canonical `brep/<id>.brep` Derived Results under test.
- Source Revision Snapshot binding for Boolean fuse, verified against the
  open bundle revision before promotion.
- Canonical promotion plus geometry replay for Boolean fuse: the Derived
  Result is removed and `reload_and_recompute_geometry` restores the same
  bytes without changing canonical state.
- Structured worker failure: a `brep_invalid` fixture failure reaches the CLI
  diagnostic, preserves `manifest.json` and `transactions.log`, and creates
  no Derived Result.
- CLI shell canonical intent persistence, source Revision Snapshot and base
  dependency binding, promoted artifact provenance, and fixture-backed replay
  without canonical-state changes (`crates/cli/tests/fast_native_contracts.rs`).
- Exact canonical identity wins over a legacy `*-base` role, including after
  bundle reload (`crates/persistence/tests/history.rs`).
- Already fast before this change: transform request validation, missing-base
  rejection before worker execution, staged extrude acceptance, fake-worker
  failure containment, protocol request/response schema validation, and
  worker-evidence incompatible edge reporting.

## Native-only

- Physical BREP validity, topology, tessellation, export bytes, viewport
  scenes, and byte-difference assertions. Reason: requires the Geometric
  Kernel to compute real geometry.
- Real OCCT/libslvs process readiness, handshake, pinned source commits,
  binary fingerprints, and execution manifests. Reason: proves the pinned
  worker supply chain and disposable process boundary.
- Solver behavior, including stable sketch IDs and attached failure
  diagnostics. Reason: requires the real libslvs worker.
- 324-hole Boolean stress, cooperative cancellation progress, release
  authorization, licensing/source-offer, performance evidence, and
  tamper-evident bundles. Reason: supply-chain, release, and performance
  gates by design.
- Physical shell/finishing geometry and real-worker replay. Reason: topology,
  tessellation, and pinned Geometric Kernel behavior require native OCCT;
  canonical shell persistence and fixture-backed replay are covered above.
- Worker-derived edge provenance through real inspection. Reason: existing
  fast tests cover incompatible evidence and validation, but selected-edge
  serialization against real topology still needs OCCT inspection.
