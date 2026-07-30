# Historical Edits And Incremental Recomputation Spike

Status: representative selected-worker integration spike completed 2026-07-30.
This is a current-machine/container baseline, not a product performance budget.

## Question

Can a historical edit invalidate only its dependency descendants, preserve an
unrelated branch and explicitly stale last-valid output, stop at the first
failure, and recover without a partial transaction?

## Method

The disposable Rust source is
`/tmp/opencode/threeterm-historical-recompute-spike/main.rs`. It is an
in-memory, symbolic host-state model. It represents the feature DAG
`a -> b -> c` plus independent branch `u -> v`; it does not call OCCT,
`libslvs`, a worker process, filesystem staging, or a terminal UI.

It was compiled and run on this host with:

```sh
cd /tmp/opencode/threeterm-historical-recompute-spike
rustc --edition 2024 main.rs -O -o recompute-spike && ./recompute-spike
```

## Observations

The run passed these assertions:

1. Editing `a` discovered dirty set `[a, b, c]`; independent `u, v` were not
   included.
2. Editing `b` to the symbolic invalid value recomputed `[b, c]`, retained
   `b:good` as `stale-last-valid`, blocked `c`, and left `u, v` current.
3. Rejecting an unknown feature preserved the exact revision and transaction
   log, exercising atomic rejection.
4. A staged result labeled with source revision 0 was rejected after the edit
   advanced the document revision. A cancellation path discarded the staged
   result without mutation.
5. Correcting `b`, suppressing it, and restoring the named
   `before-historical-edit` revision each produced the expected current,
   blocked, and recovered states.
6. Replaying the same symbolic command sequence produced equal feature state
   and transaction sequence.

The final formatted complete run reported `elapsed_us=58`. This measures construction,
cloning, graph traversal, assertions, and console output in the five-node
symbolic fixture. It is not a latency measurement for OCCT, `libslvs`, native
workers, IPC, staging/promotion, disk persistence, or realistic feature
graphs, and it must not become a product budget.

## Implications

The exercised host model is consistent with the closed decisions in
[Define history promises and timeline topology](https://github.com/rafaelromao/threeterm/issues/4),
[Define persistent domain-event granularity](https://github.com/rafaelromao/threeterm/issues/29),
[Define failure handling after historical edits](https://github.com/rafaelromao/threeterm/issues/32),
and [Define command parity between TUI and headless modes](https://github.com/rafaelromao/threeterm/issues/33):

- derive a stable dirty set from the canonical feature dependency graph, not
  from chronology alone;
- stage recomputation output and atomically promote it only when its source
  revision remains current;
- persist an accepted edit transaction even when downstream recomputation
  fails, marking retained output explicitly `stale-last-valid` and stopping
  that affected branch;
- preserve unrelated current-valid branches; and
- treat correction, suppression, and named-revision restoration as explicit
  subsequent transactions.

## Selected-Worker Integration

### Environment And Fixture

The authorized disposable integration source and artifacts are retained at
`/tmp/opencode/threeterm-historical-worker-spike/`. Rootless Podman ran a
fresh `archlinux:latest` container on host kernel `7.1.4-arch1-1` x86_64. No
host packages were installed.

- OCCT: Arch `opencascade` `1:7.9.3-3`.
- Solver: SolveSpace `libslvs` v3.2 source at commit
  `27b6a080c8b669421bd4d444650c3b8eddec5687`, built library-only in the
  container.
- Host: Rust `1:1.97.1-1`; workers: C++ executables. The host never linked
  OCCT or `libslvs`.
- Protocol: each disposable worker returned a newline-delimited completion
  record containing staged artifact path, byte count, peak RSS, and SHA-256.
  The Rust host independently recomputed that SHA-256, invoked a fresh
  worker-side semantic verifier, compared source revision, and then renamed
  the staged file into a same-filesystem canonical directory.
- Geometry branch: OCCT box `base(width, 50, 12)` followed by a BRep-read
  cylinder cut `hole(base)`. Sketch branch: `libslvs` solves a vertical
  line with a point-to-point dimension; a separate OCCT box represents its
  geometric dependent. The branches are independent.

Run command:

```sh
podman run --rm \
  -v /tmp/opencode/threeterm-historical-worker-spike:/work:Z \
  -v /tmp/opencode/threeterm-solver-spike/solvespace:/solvespace:ro,Z \
  docker.io/archlinux:latest bash /work/run.sh
```

### Results

The final run is preserved in
`/tmp/opencode/threeterm-historical-worker-spike/results/results.json`.

| Observation | Result |
| --- | ---: |
| Initial OCCT base worker | 8.823 ms |
| Initial OCCT BRep-read/hole worker | 15.053 ms |
| Initial `libslvs` worker | 3.769 ms |
| Five geometry historical edits, mean base plus hole | 23.463 ms |
| Five sketch historical edits, mean `libslvs` plus OCCT dependent | 12.071 ms |
| OCCT worker peak RSS | 16,776 KiB |
| `libslvs` worker peak RSS | 7,300 KiB |
| Cumulative dirty features | 20 |
| Cumulative real recomputations/promotions | 23 / 23 |
| Cumulative clean-branch cache hits | 15 |
| Rejected staged results | 3 |

Five early geometry edits dirtied exactly the OCCT base and hole branch while
retaining the sketch branch as one cache hit each. Five sketch edits dirtied
the `libslvs` node plus its OCCT dependent while retaining the two-feature
base/hole branch as two cache hits each. The 23 recomputations are three
initial results plus those 20 dirty nodes.

The host rejected and removed a syntactically complete stale OCCT artifact
after a concurrent revision advance. It also rejected and removed a
SHA-256-correct but non-BRep artifact after OCCT readback/validity verification,
and rejected a crashing `libslvs` worker (exit 99) without a promotion. The
existing canonical output was not overwritten in any of these paths.

Cancellation was measured from Rust-host `SIGTERM` request to worker exit:

| Worker behavior | OCCT | `libslvs` |
| --- | ---: | ---: |
| Cooperative test loop | 16.641 ms | 3.331 ms |
| Ignored `SIGTERM`, host forced `SIGKILL` after 20 ms | 23.676 ms | 23.589 ms |

The cooperative loops are deliberate worker controls, not claims that OCCT
or `libslvs` interrupt a native monolithic operation. They demonstrate the
worker lifecycle, output discard, and forced-stop bound in this fixture.

The host serialized only canonical revision 12 inputs (`base=74`,
`sketch=16`, and transaction strings), then constructed fresh worker results.
The final OCCT base BRep, OCCT BRep-read/hole BRep, and `libslvs` artifact had
equal SHA-256 hashes to independently recomputed reference artifacts in this
same container/version run.

## Limits And Follow-up

This resolves the ticket's representative early-edit question. It does not
close the selected kernel/solver/package/protocol validation gates. In
particular, the following remain required before production implementation:

- larger and more varied feature DAGs, complex Booleans/fillets/shells/lofts,
  and the 50/200/1,000-constraint solver corpus;
- persistent warm-worker policy, full process/container cold-start timings,
  sample distributions, and representative hardware baselines;
- cancellation inside arbitrary monolithic native calls, rather than the
  worker control loops used here;
- hostile IPC framing, path traversal/ownership, size/deadline limits, and
  host/worker death cleanup; and
- replay across workers, packages, Linux targets, locales, parallel settings,
  schema migrations, and OCCT/`libslvs` version changes. Same-container hashes
  are not a cross-version or cross-platform determinism guarantee.

No feature-geometry or solver claim is made by this note.
