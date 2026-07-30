# Serialization, Reload, and Replay Spike Protocol

Status: selected-model execution completed 2026-07-30. This is a disposable
same-container baseline, not a project-format selection or compatibility promise.

## Canonical Fixture

A minimal project fixture must contain:

- manifest: project schema, transaction schema, kernel, solver, export, and feature-module versions;
- canonical feature graph with stable object/feature IDs;
- append-only versioned command transactions;
- named revision metadata and parentage;
- optional snapshot with declared graph/journal position and integrity hash;
- derived geometry/cache entries explicitly marked non-authoritative.

## Required Cases

1. create sketch, constrain it, extrude, hole, fillet, export;
2. edit an earlier dimension and replay affected future;
3. invalid later feature with preserved last-valid state;
4. undo, divergence, named-revision recovery;
5. interrupted atomic write;
6. corrupt transaction, manifest, snapshot, and cache;
7. unknown command/event schema;
8. missing or incompatible feature-module metadata;
9. stale derived geometry;
10. repeated replay on same version and cross-version replay.

## Measurements

- time to manifest, editable graph, first geometry, and fully rebuilt derived state;
- canonical graph equality, transaction identity, stable-ID equality, geometry tolerance equality, error equality, and export validity;
- project size and transaction/snapshot growth;
- recovery diagnosis and lost-work bound;
- replayed node count, cache hits, invalidation scope, memory, and cancellation latency.

## Exit Criteria

Select snapshot authority, compatibility policy, recovery behavior, and project format only after the selected kernel and solver can execute this corpus. Do not claim replay determinism from JSON round-tripping alone.

## Selected-Model Execution

The disposable Rust host at
`/tmp/opencode/threeterm-serialization-replay-spike/host.rs` ran in a fresh
rootless `archlinux:latest` Podman container. It rebuilt the existing C++ OCCT
and `libslvs` workers from the selected-worker historical-edit spike, rather
than linking either library into Rust. Environment versions were OCCT
`1:7.9.3-3`, Rust `1:1.97.1-1`, and SolveSpace `libslvs` v3.2 at
`27b6a080c8b669421bd4d444650c3b8eddec5687`.

The host wrote a deliberately small sealed canonical container: a container,
manifest, and transaction schema version; pinned worker metadata; immutable
revision; ordered versioned command transactions; and an explicitly
non-authoritative snapshot/Derived Result section. The four transactions
created base and sketch inputs, then edited both. Replay reconstructs the
canonical inputs from transactions before invoking fresh workers; no BRep,
solver output, cache, or progress record is canonical.

The run passed these checks:

- Reload reconstructed the identical revision-four canonical document and four
  transaction records; two complete independent replays generated equal SHA-256
  hashes for OCCT base, OCCT hole, and `libslvs` artifacts in the same pinned
  container.
- The atomic-save path wrote and synced a temporary file, then renamed it. A
  deliberately interrupted, synced partial temporary write left the prior
  sealed project intact and reloadable.
- Tampering with a command without recomputing integrity was rejected. A
  correctly sealed malformed container was rejected by magic validation; a
  correctly sealed future transaction schema was rejected as unsupported; and
  a correctly sealed incompatible OCCT manifest was rejected before worker use.
- An OCCT malformed staged artifact had a valid completion/hash but failed
  worker semantic verification. It was not promoted; the last valid hole BRep
  remained, and the persisted Derived Result became explicit
  `stale-last-valid` with an artifact-semantic-invalid diagnostic.
- Changing the persisted base cache worker fingerprint invalidated it. A fresh
  validated OCCT execution replaced that cache entry only after promotion.

Raw result summary, sealed fixtures, and container environment records are at
`/tmp/opencode/threeterm-serialization-replay-spike/results/`; the run command
is in its `run.sh`. Its `results.json` records all seven assertions. The harness
reuses the actual worker sources from
`/tmp/opencode/threeterm-historical-worker-spike/`, but is a separate
disposable persistence artifact.

## Limits

This validates one selected canonical feature-graph plus versioned-transaction
model. It does not compare or select an event-sourced alternative, select a
shipping container/layout, test named-revision divergence, export, large
feature/constraint corpora, warm workers, hostile path/framing/size attacks,
filesystem crash durability beyond the tested rename interruption, migrations,
or cross-version/platform/locale/parallel determinism. Equal hashes are only a
same-container result and are not an OCCT or `libslvs` compatibility guarantee.
