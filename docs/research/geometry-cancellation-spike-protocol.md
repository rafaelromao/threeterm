# Geometry Cancellation Spike Protocol

Status: executed for the Boolean-pattern architecture fixture and representative operation-specific coverage.

## Worker Contract

- Host sends immutable command transaction, input feature graph revision, and request ID to a disposable worker.
- Worker emits progress, cancellation acknowledgement, structured result, or structured failure.
- Host retains last valid graph/derived-state revision until a complete validated result commits.
- Host requests cooperative cancellation first. If acknowledgement/result is absent after configured grace period, host terminates worker.
- Terminated worker output is discarded; host records request ID, stage, elapsed time, last progress, exit signal, and reproducible inputs.

## Fixtures

1. high-count Boolean pattern;
2. shell/thickening on a complex filleted body;
3. loft with many sections;
4. fine tessellation for STL/3MF;
5. STEP export of multi-body model;
6. deliberately nonterminating worker control.

## Measurements

- cancel-request to acknowledgement latency;
- cancel-request to worker exit latency;
- progress freshness;
- host responsiveness;
- last-valid-state preservation;
- partial-artifact cleanup;
- retry/replay success;
- diagnostic completeness;
- worker startup and serialization overhead.

## Exit Criteria

- cooperative cancellation proves safe on operations that advertise support;
- forced worker termination proves bounded recovery and no host/document corruption;
- cancellation policy receives operation-specific grace periods from measured p95 latency;
- no partial geometry or export can become authoritative.

## Execution

### Environment

- Rootless Podman isolated Arch Linux container
- Direct OCCT route: Arch `opencascade` 1:7.9.3-3
- Headless application route: Arch `freecad` 1.1.3, which includes OCCT
- Fixture: a 120 x 120 x 20 solid with a sequential 324-hole Boolean cut pattern
- Host protocol: newline-delimited structured events, immutable request input, a known-good revision retained by the host, staged BREP output, and commit only after a structured completion event and validation
- Samples: 10 cooperative cancellations per route, one ignored-cancellation control per route, and one successful retry per route

The worker checked cancellation between kernel calls. This measures safe adapter-level cancellation for a decomposable operation. It does not prove that an arbitrary monolithic OCCT or FreeCAD call observes cancellation internally.

### Results

| Route | Cooperative cancel-to-exit range | p95 (nearest rank) | Ignored-cancel recovery | Retry |
| --- | ---: | ---: | ---: | --- |
| Direct OCCT worker | 7.198-7.291 ms | 7.291 ms | SIGKILL after 250 ms grace; exited in 251.208 ms | Valid staged result committed in 249.470 ms |
| FreeCAD command worker | 15.306-15.394 ms | 15.394 ms | SIGKILL after 250 ms grace; exited in 253.310 ms | Valid staged result committed in 383.490 ms |

All 20 cooperative runs acknowledged cancellation and exited without forced termination. Both forced-stop controls preserved the byte-identical last-valid revision, left no authoritative result, removed any staging path, kept the host responsive, and recorded request ID, route, mode, stage, elapsed time, last progress, exit code/signal, and reproducible fixture inputs. Both retries produced BREP artifacts only after a structured completion event and pre-commit validation.

The FreeCAD command route writes startup/license text to stdout and can return process exit code zero after a Python script exception. Its adapter must therefore separate protocol events from launcher noise and must never accept process exit status alone as success. The retry also emitted a warning for the `.brep.partial` staging suffix while writing recognizable BREP content; production staging should use a supported suffix in a private staging directory or call an explicit writer, then rename atomically after validation.

Disposable worker sources, raw measurements, and retry artifacts are retained at `/tmp/opencode/threeterm-geometry-spike/cancellation/` for this session.

## Findings

1. Both candidate routes can satisfy the approved cooperative-first policy when ThreeTerm owns a cancellable operation boundary between kernel calls.
2. Neither route should be trusted to interrupt every monolithic native call. A disposable process is the required hard-stop boundary for calls without demonstrated progress-range propagation.
3. The host, not the geometry runtime, must own authoritative document state. Workers receive immutable inputs and return staged artifacts; cancellation, timeout, crash, malformed output, or missing terminal events cannot mutate the retained revision.
4. A 250 ms experimental grace bounded both ignored-cancel controls, but it is not a product-wide budget. Each operation class needs measured cancellation p95 and progress freshness before its production grace is selected.
5. The follow-up feature-coverage corpus applied this contract to shell, loft, draft, circular pattern, and fine tessellation. It did not demonstrate interruption inside monolithic native calls or export writers.

## Consequences

- Keep direct OCCT and headless FreeCAD viable for kernel selection on cancellation architecture.
- Require structured terminal events plus validated artifacts for success; process exit code is diagnostic input, not a commit condition.
- Route monolithic or unproven long calls through disposable workers. Keep measured-safe low-latency work in process where that simplifies the system.
- Feed operation classification, grace periods, stale-result handling, and progress UX into [Choose asynchronous operation boundaries](https://github.com/rafaelromao/threeterm/issues/49).

## Operation-Specific Follow-Up

Five cooperative samples per operation and route were run in disposable workers. Every sample acknowledged cancellation between complete kernel calls, exited without force, left no authoritative result, and removed its staging path.

| Route | Shell p95 | Loft p95 | Draft p95 | Pattern p95 | Fine tessellation p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Direct OCCT worker | 7.420 ms | 7.296 ms | 3.224 ms | 31.473 ms | 31.529 ms |
| FreeCAD command worker | 15.487 ms | 15.429 ms | 15.415 ms | 113.630 ms | 64.053 ms |

These nearest-rank p95 values over five samples are classification evidence, not production budgets. The checks occur between monolithic calls, so forced worker termination remains the hard-stop policy when any one call does not return. Full feature, degeneracy, replay, and resource findings are in [`geometry-operation-spike.md`](./geometry-operation-spike.md); raw records are in `/tmp/opencode/threeterm-geometry-spike/coverage/results/measurements.json`.
