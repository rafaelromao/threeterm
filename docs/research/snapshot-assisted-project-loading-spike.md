# Snapshot-Assisted Project Loading Spike

Status: disposable current-machine fixture completed 2026-07-30. This is not
production code or a product-performance claim.

## Question

Can representative sealed `.threeterm` Project Generations safely reach
metadata, a Revision Snapshot, and a derived-state decision when an optional
Persisted Checkpoint or Derived Result is valid, absent, stale, corrupt, or
incompatible, while preserving the Canonical Transaction Log as authority?

## Harness

`/tmp/opencode/threeterm-loading-spike/main.rs` is a std-only Rust harness. It
creates sealed directory bundles with `manifest.json`, canonical
`transactions.ndjson`, an optional `checkpoints/3.json`, and an optional binary
Derived Result plus JSON metadata. It implements SHA-256 (including an `abc`
test vector), authenticates the log before reading accelerators, and emits
structured `code:layer:detail` diagnostics. It does not invoke OCCT, `libslvs`,
or a real ThreeTerm worker.

Run:

```sh
cd /tmp/opencode/threeterm-loading-spike
rustc --edition 2024 main.rs -O -o loading-spike
./loading-spike
```

The fixture has three canonical version-1 command transactions: creation,
hole, and accepted historical-edit failure. The manifest requires the pinned
synthetic worker identity `occt-7.9.3;libslvs-3.2` and supports schema epochs
one and two, matching the selected two-epoch migration policy.

## Observations

- A valid current Project Generation authenticated first, accepted its matching
  Persisted Checkpoint, replayed zero tail records, and accepted its
  current-valid Derived Result.
- Missing, malformed, and stale checkpoints produced
  `checkpoint-missing` or `checkpoint-discarded`; each replayed all three
  authenticated records. None repaired, replaced, or overrode canonical data.
- A corrupt artifact and a Derived Result with mismatched worker fingerprint
  each produced `derived-cache-miss` and `rebuild-required`, while the
  authenticated document remained editable. This is cache invalidation, not
  canonical incompatibility.
- A persisted `stale-last-valid` artifact was retained only with the explicit
  `stale-last-valid` diagnostic and was described as non-authoritative and
  excluded from validation/export.
- A current generation whose NDJSON no longer matched the manifest SHA-256 was
  rejected. A complete authenticated `.previous` generation was then opened,
  with `recovered-previous`; no files were mixed between generations.
- A future manifest epoch and an incompatible required worker fingerprint were
  rejected before checkpoint or Derived Result use, with
  `schema-unsupported` and `worker-incompatible` diagnostics.
- A previous-epoch bundle was authenticated, copied to a sealed
  `.pre-migration-backup`, transformed by the harness's deterministic host-only
  migration fixture, atomically replaced with epoch two, and reopened from its
  canonical log/checkpoint. The prior generation remained as `.previous`.
- Two independent opens of the valid fixture both used its checkpoint. They are
  reported as cold/warm filesystem observations only; the harness deliberately
  has no persistent loader cache.

## Current-Machine Evidence

On this machine with Rust `1.97.1`, optimized fixture opens were about 11-58 us
per tiny three-record bundle; the two valid independent opens were 38 us and
36 us in the recorded run. The process peak RSS was 2,448 KiB. These numbers
include only this harness's process, tiny local files, SHA-256, and host
filesystem cache effects. They exclude real geometry, worker startup/IPC,
artifact validation, UI, realistic history size, concurrency, and distributions
across hardware. They must not become product budgets.

## Consequences

The exercised behavior is consistent with [Define snapshot and replay
policy](https://github.com/rafaelromao/threeterm/issues/31), [Choose the saved
project format](https://github.com/rafaelromao/threeterm/issues/44), [Define
project schema migration policy](https://github.com/rafaelromao/threeterm/issues/45),
[Define persistent domain-event granularity](https://github.com/rafaelromao/threeterm/issues/29),
[Define incremental recomputation semantics](https://github.com/rafaelromao/threeterm/issues/36),
and [Define failure handling after historical edits](https://github.com/rafaelromao/threeterm/issues/32):
authenticate canonical authority first; treat checkpoints and Derived Results
as discardable accelerators; recover only a whole prior Project Generation;

## Limits

This fixture proves neither a shipping loader nor durability after power loss.
It does not test real JSON canonicalization/parser hardening, fsync ordering,
atomic rename interruption at every step, hostile paths/sizes, partial-tail
repair, named-revision divergence, real OCCT/libslvs replay, geometry semantic
validation, worker cold/warm policy, large projects, platform/version/locale
determinism, or actual migration semantics. The migration transforms only a
synthetic canonical fixture and must not be treated as production migration
machinery.
