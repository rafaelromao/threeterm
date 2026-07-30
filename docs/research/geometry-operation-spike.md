# Geometry Operation Spike

Status: complete representative feature-coverage spike; not a kernel selection.

## Goal

Exercise representative exact geometry, export, degeneracy, cancellation, resource, and deterministic-replay behavior against the direct OCCT and headless FreeCAD candidate routes after ThreeTerm's complete MVP feature envelope was fixed.

## Environment

- Rootless Podman isolated Arch Linux containers
- Direct OCCT route: Arch `opencascade` 1:7.9.3-3
- Headless application route: Arch `freecad` 1.1.3, which includes OCCT
- Internal parallelism was not enabled by the fixtures.
- Initial fixture: 80 x 50 x 12 box; cylinder fuse; cylindrical cut/hole; one-edge fillet; mirror; three-copy linear pattern; closed-profile revolve; two-profile solid loft; STEP and STL export
- Coverage fixture: one-edge chamfer; one-open-face shell; one-side neutral-plane draft; four-copy circular pattern; FreeCAD two-body 3MF; five degenerate cases; a combined shell/pattern replay model
- Cancellation fixture: five samples per route for shell, 24-section loft, draft, overlapping circular pattern, and fine tessellation
- Every accepted ordinary feature result passed `BRepCheck_Analyzer` or FreeCAD `Shape.isValid()`.

Timings are container observations, not product budgets or controlled benchmarks. Operation timings below are one representative run. Replay ranges cover ten cold worker processes per route.

## Ordinary Feature Coverage

| Operation | Direct OCCT | FreeCAD headless | Result |
| --- | ---: | ---: | --- |
| Box | 144 us | 130 us | Valid |
| Cylinder fuse | 1,527 us | 2,440 us | Valid |
| Cylinder cut/hole | 2,477 us | 2,755 us | Valid |
| Fillet | 1,325 us | 1,961 us | Valid |
| Chamfer | 2,098 us | 3,586 us | Valid solid |
| Mirror | 225 us | 294 us | Valid |
| Three-copy linear pattern | 5,897 us | 9,786 us | Valid |
| Four-copy circular pattern | 6,015 us | 11,828 us | Valid four-solid compound |
| Revolve | 122 us | 292 us | Valid |
| Loft | 2,331 us | 2,134 us | Valid |
| Open-face shell, -2 mm | 4,843 us | 5,343 us | Valid solid; volume 7,152 mm3 |
| Neutral-plane side draft, 5 degrees | 1,019 us | 111,997 us | Valid solid |
| STEP export | 3,360 us; 26,701 bytes | 661 us; 1,640 bytes | Successful initial fixture |
| STL export | 3,200 us; 14,284 bytes | 7,665 us; 290,460 bytes | Successful initial fixture |

The draft volumes differ because the direct shape-level and FreeCAD PartDesign fixtures selected faces through route-specific topology ordering. This proves each route can execute a neutral-plane draft policy; it is not a geometric equality comparison between adapters.

File-size differences are not quality conclusions. The fixture, exporter defaults, schema, tolerance, and tessellation policy differ.

## Representative Degenerate Inputs

Both routes produced the same high-level classifications:

| Case | Observed behavior | Product consequence |
| --- | --- | --- |
| Disjoint Boolean cut | Returned the unchanged valid 24,000 mm3 box with no native diagnostic | Compare operation intent and topology/mass change; validity alone cannot distinguish a legitimate no-op from a misplaced tool. |
| Loft with coincident sections | Reported a valid solid with zero volume and no native diagnostic | A zero-volume or dimensionally collapsed result must fail ThreeTerm's solid policy even when the kernel analyzer accepts it. |
| Shell offset larger than the body | Returned the unchanged valid 24,000 mm3 box with no native diagnostic | Detect no-op shell results and reject impossible thickness before commit. |
| Draft near 90 degrees | Direct OCCT returned `algorithm_not_done`; FreeCAD produced a null/invalid shape | Normalize both into a structured operation failure; FreeCAD process exit status is not sufficient evidence of failure or success. |
| Circular pattern with zero step | Collapsed to one valid 576 mm3 solid with no native diagnostic | Validate pattern count and nonzero transform before dispatch; do not accept a valid collapsed result silently. |

These cases are representative boundary probes, not an exhaustive robustness corpus. They establish that `isValid`/`BRepCheck_Analyzer` is necessary but insufficient: semantic preconditions, expected cardinality, positive volume, and no-op detection belong in ThreeTerm's validation adapter.

## 3MF Multi-Body Export

- Standard OCCT exchange has no 3MF writer. The direct route therefore reports the capability unsupported and requires the previously identified separate `lib3mf` boundary downstream of a canonical mesh.
- FreeCAD exported two source features to a 5,356-byte ZIP-based 3MF containing two `<object>` resources and two build items.
- All ten cold FreeCAD exports had identical uncompressed entry hashes and the same object/build counts. ZIP container bytes were not used as the oracle because ZIP timestamps vary.
- The source labels `first_body` and `second_body` were absent from the model XML. The adapter must supply and verify required naming/metadata rather than assume FreeCAD preserves feature labels.
- This spike inspected ZIP structure and model semantics; it did not run the output through an independent 3MF conformance validator, slicer matrix, or unit/material round-trip.

## Operation-Specific Cancellation

The worker contract from [`geometry-cancellation-spike-protocol.md`](./geometry-cancellation-spike-protocol.md) was applied to five operation classes. Each worker checked cancellation between complete kernel calls. All 50 samples emitted a structured cancellation event, exited cooperatively, left no authoritative result, and removed the staging path.

| Route | Shell p95 | Loft p95 | Draft p95 | Pattern p95 | Fine tessellation p95 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Direct OCCT worker | 7.420 ms | 7.296 ms | 3.224 ms | 31.473 ms | 31.529 ms |
| FreeCAD command worker | 15.487 ms | 15.429 ms | 15.415 ms | 113.630 ms | 64.053 ms |

The values are nearest-rank p95 over five samples, so they classify operation behavior but are too small a sample for production grace periods. The pattern outlier also shows why one global grace is unjustified.

This is adapter-level cooperative cancellation of decomposable repeated work. It does not prove that a monolithic shell, loft, draft, Boolean, tessellation, STEP, or 3MF native call observes a signal internally. The previously measured forced-stop worker boundary remains required for any monolithic call that misses its operation-specific grace period.

## Deterministic Replay and Resource Observations

The combined replay fixture was executed in ten cold processes per route.

| Observation | Direct OCCT | FreeCAD headless |
| --- | ---: | ---: |
| Successful cold replays | 10/10 | 10/10 |
| Geometry metric variants | 1 | 1 |
| Raw BREP hash variants | 1 | 1 |
| Raw STL hash variants | 1 | 1 |
| Raw STEP hash variants | 2 | 5 |
| Metadata-normalized STEP hash variants | 1 | 1 |
| End-to-end elapsed range | 81.646-92.846 ms | 384.830-462.454 ms |
| Peak process RSS range | 39,892-40,980 KiB | 132,952-133,608 KiB |

The stable geometry facts were validity, solid/face/edge counts, and volume. The direct replay fixture consistently reported 3 solids, 35 faces, 166 edge occurrences, and 9,168 mm3; FreeCAD reported the same solids/faces/volume and 83 unique wrapped edges. Route-specific traversal conventions make raw edge counts unsuitable for cross-adapter equality without normalization.

STEP raw hashes varied because file metadata varied; normalizing the `FILE_NAME` header made each route stable across its ten runs. This does not imply byte stability across OCCT/FreeCAD versions, distributions, CPU vendors, locales, thread settings, or route changes. It also does not prove topological-reference determinism.

RSS is peak process resident set for the complete fixture, including startup and exports, not incremental memory attributable to one operation. FreeCAD's observed process envelope was about 3.3 times the direct worker's in this fixture.

## Findings

1. Both routes cover the tested MVP geometry operations, including chamfer, shell, neutral-plane draft, and circular pattern, without selecting either route.
2. Direct OCCT requires ThreeTerm to own feature graph, transactions, errors, persistent references, semantic result checks, and export adapters. FreeCAD provides higher-level document features but carries a larger application runtime and route-specific object/document behavior.
3. Exact B-rep and mesh export are distinct: direct OCCT's STL writer required `BRepMesh_IncrementalMesh`; STEP operated on exact B-rep.
4. OCCT's Arch CMake package imports visualization targets that require VTK even for a core-only fixture. Direct library linking avoided that transitive dependency.
5. OCCT 7.9 uses `TKDESTEP` and `TKDESTL`; version-pinned build fixtures remain required.
6. FreeCAD `Mesh.export` requires document objects rather than raw `Part.Shape` values. Its 3MF path preserves separate objects/build items in this fixture but not their labels.
7. Kernel validity cannot enforce ThreeTerm's printable-solid or feature-intent policy. Several degenerate cases silently returned valid, unchanged, collapsed, or zero-volume results.
8. Both routes fit the cooperative-first, disposable-worker cancellation architecture at adapter boundaries. Neither may be trusted to interrupt arbitrary monolithic native calls.
9. Same-version, same-container replay was geometrically stable for this fixture. Raw STEP bytes were not stable, and broader determinism remains unproven.

## Not Yet Established

- exhaustive Boolean, loft, shell, draft, pattern, fillet, and chamfer degeneracy behavior;
- independent 3MF conformance, units, labels, materials, multi-component semantics, and slicer compatibility;
- cancellation inside monolithic native calls or export writers;
- deterministic behavior across versions, distributions, CPU vendors, locales, parallel settings, serialization/reload, and route changes;
- persistent-reference survival and topology normalization;
- project-document incremental recomputation, package/startup benchmarks, and long-duration leak behavior;
- export-normal, units, tolerance, topology, and mesh-quality policy.

These are separate selection and implementation gates. They do not invalidate the representative feature-coverage answer, and they must not be inferred from this spike.

## Assets

Disposable fixture source, raw measurements, and outputs are retained for this session at `/tmp/opencode/threeterm-geometry-spike/`:

- `main.cpp`, `CMakeLists.txt`, `freecad_spike.py`, `output/`, and `output-freecad/` for the initial operation slice;
- `coverage/occt_coverage.cpp`, `coverage/freecad_coverage.py`, and `coverage/run_coverage.py` for complete feature coverage;
- `coverage/results/measurements.json` for the ten replay runs and 50 cancellation samples (SHA-256 `e1cee77f434cc1ea64ae7793658e06c831eb4912bbe7eccc3b371b4ecb8f803b`);
- `coverage/results/runs/` for BREP, STL, STEP, and 3MF outputs;
- `cancellation/` for the earlier Boolean-pattern containment fixture.

## Sources

- OCCT kernel and exchange evidence: [`geometry-toolchain-options.md`](./geometry-toolchain-options.md)
- Cancellation contract and containment evidence: [`geometry-cancellation-spike-protocol.md`](./geometry-cancellation-spike-protocol.md)
- OCCT package metadata: <https://archlinux.org/packages/extra/x86_64/opencascade/>
- FreeCAD package metadata: <https://archlinux.org/packages/extra/x86_64/freecad/>
- lib3mf project: <https://github.com/3MFConsortium/lib3mf>
