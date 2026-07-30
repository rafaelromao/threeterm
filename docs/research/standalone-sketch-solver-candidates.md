# Standalone Sketch Solver Candidates

- Status: supports the selected solver strategy in [Select the sketch constraint-solving strategy](https://github.com/rafaelromao/threeterm/issues/25).
- Researched: 2026-07-30.
- Scope: open-source, credibly embeddable sketch solvers compared with the existing FreeCAD Sketcher/PlaneGCS evidence. Commercial components are not scored without evaluation access and redistribution terms.

## Selected Strategy

ThreeTerm will use SolveSpace `libslvs` behind a disposable worker. The product owner accepts either GPLv3 distribution or procuring a commercial `libslvs` license; the selected distribution must use one of those routes and preserve its applicable obligations. A process boundary is for failure containment, not a license workaround.

The worker accepts immutable ThreeTerm sketch entities and constraints identified by stable ThreeTerm IDs. It rebuilds `libslvs` inputs, maps solver handles back to those IDs, returns normalized status, degrees of freedom, and optional related constraint IDs, and copies solved coordinates only on a successful solve. It must discard all uncommitted output when terminated after a missed deadline; `libslvs` has no reviewed public cancellation, progress, or interrupt API. Failed-constraint calculation remains opt-in because upstream documents an approximately one-solve-per-constraint cost.

This selection does not establish full solver fitness. Before production implementation relies on it, the worker must pass the shared corpus for lines, arcs, circles, tangent/equal/concentric/symmetric constraints, 50/200/1,000-constraint solve-after-edit and drag-equivalent cases, redundancy/conflict/nonconvergence, stable ID mapping, cancellation/restart/no-partial-commit behavior, nonlinear and ambiguous branch stability, supported-target determinism, and stripped-package/startup/RSS/offline-install measurements.

PlaneGCS extraction is rejected. FreeCAD Sketcher is not the selected solver route; it remains comparison evidence only, with a substantially larger application runtime and diagnostics that were not stable through the tested scripting property.

## Recommendation

Do **not** extract PlaneGCS for ThreeTerm. It is a richer solver implementation, but its current upstream boundary is a FreeCAD Sketcher C++ shared module, not a standalone library or stable ABI. Extraction would create a fork, an Eigen/FreeCAD build seam, and a new API/diagnostics/packaging maintenance obligation before product work begins.

SolveSpace `libslvs` is the selected standalone solver route. It has a small C ABI, Linux source build/install support, core mechanical-sketch entities and constraints, DOF/status/failed-constraint output, and a measured minimal fixture. Its GPLv3 or commercial-license route is accepted by the product owner. It has no public cancellation/progress API, limited documented conflict explanation, and upstream describes the solver as heavily coupled to SolveSpace; run it in a disposable worker and retain ThreeTerm constraint IDs at the adapter boundary.

FreeCAD Sketcher remains the better-supported route for a broad, rich constraint corpus only when ThreeTerm deliberately accepts the FreeCAD runtime. The prior headless spike demonstrated fully constrained rectangle/circle cases and redundancy detection, but its diagnostics surfaced through output rather than a stable scripting property. It does not justify treating PlaneGCS as a supported standalone dependency.

No candidate has demonstrated ThreeTerm's full corpus, interactive latency at 50/200/1,000 constraints, branch stability, worker cancellation latency, cross-version determinism, or a minimal distributable package. These are selection-gate experiments, not facts established here.

## Candidate Comparison

| Criterion | SolveSpace `libslvs` | PlaneGCS extraction | FreeCAD Sketcher worker |
| --- | --- | --- | --- |
| Upstream integration maturity | Public C header, shared-library CMake target, installed header/library; upstream still calls solver application-coupled. | Source-reusable implementation, but no independent repository, release, package, or C ABI. | Mature application module and documented headless command process; not a solver SDK. |
| API, binding, language boundary | C ABI in `slvs.h`; natural Rust/C++ FFI and easy worker protocol. Caller owns arrays/handles and C allocation for failed handles from `Slvs_SolveSketch`. | C++ classes and raw `double *` parameter ownership. `SketcherExport` ties visibility to FreeCAD; current target links `Part` and `FreeCADApp`. | Python-facing `Sketcher::SketchObject` in a FreeCAD process. Python/application lifecycle is the boundary. |
| Geometry/constraint corpus | Points, workplanes, lines, circles, arcs, cubics; coincidence, dimensions, equal/equal radius, symmetry, horizontal/vertical, angle, parallel/perpendicular, tangent, dragged geometry, and related ratios/differences are declared in the public header. | Source exposes lines, circles, arcs, conics, B-splines and a substantially wider family of geometric/derived constraints. Mapping them to a standalone product API is unproven. | Existing spike covers rectangle/circle plus redundant horizontal constraint only. Product supports a broad Sketcher corpus, but no complete ThreeTerm corpus measurement exists. |
| Diagnostics and conflict explanation | `result`, `dof`, and optionally failed constraint handles. The header warns failed-constraint calculation costs approximately one solve per constraint. It does not promise a minimal conflict set or prose explanation. | `diagnose()` exposes DOF, conflicting, redundant, partially redundant constraint tags, and dependent parameters. Richer source-level diagnosis, but not a released contract. | `solve()` and `FullyConstrained` observable headlessly. Prior spike saw redundancy through solver output, not a `SolverMessages` property. |
| Incremental solving | Groups permit solving a target group while older groups remain fixed. No documented dependency graph, persistent sketch object model, or public edit/invalidation API; adapter should own immutable inputs and rebuild/solve. | In-memory `System` owns constraints/subsystems and supports add/remove/clear-by-tag. No standalone persistence or stable incremental contract. | FreeCAD document recomputation can operate on touched/selected objects, but that is application-document behavior, not PlaneGCS alone. |
| Cancellation and containment | No cancel, interrupt, progress, or callback entry point appears in public `slvs.h`. Use a worker; terminate and discard uncommitted output after a deadline. | No reviewed public cancellation surface. Extraction would still need a worker or new cooperative API. | Process is naturally killable. Graceful solver cancellation has not been measured. |
| Determinism | No upstream cross-run/platform/version promise. Minimal fixture was stable for 1,000 rebuild-and-solve runs on one container, not a proof for nonlinear/ambiguous sketches. | No such promise found. Algorithm selection and Eigen version are further variables. | No such promise found. `FreeCADCmd` forces numeric locale `C`, removing one input variable only. |
| License | GPL-3.0-only repository metadata and SolveSpace states GPLv3; commercial licensing/support is offered. Legal approval is required before linking/distribution. | Files are SPDX `LGPL-2.1-or-later`; extraction also needs dependency and notice audit. | FreeCAD repository is LGPL-2.1; whole runtime carries dependency and active-content surface beyond solver licensing. |
| Linux packaging | v3.2 release ships source tarball but no Linux binary asset. Official CMake installs shared library and header. Measured local release build produced a 1,910,224-byte `libslvs.so.3.2` before stripping/package closure. | No standalone package/install target. It is compiled into `Sketcher` shared library. | Prior research records official FreeCAD 1.1.3 Linux AppImage at 820,795,896 bytes; this is a full application, not a solver package. |

## Measured Minimal `libslvs` Fixture

### Method

Rootless Podman ran `archlinux:latest`; no host packages were installed. SolveSpace v3.2 (`27b6a080c8b669421bd4d444650c3b8eddec5687`) was cloned with its pinned submodules and built as a library-only CMake configuration:

```text
cmake -S solvespace -B solvespace/build \
  -DENABLE_GUI=OFF -DENABLE_CLI=OFF -DENABLE_TESTS=OFF
cmake --build solvespace/build --parallel 2
```

The disposable harness is `/tmp/opencode/threeterm-solver-spike/libslvs_probe.c`. It constructs a fixed XY workplane plus a two-point line in a separate solve group, then applies vertical and point-to-point distance constraints. It calls `Slvs_Solve(..., 2)` with `calculateFaileds=1`.

| Case | Observed result | Interpretation |
| --- | --- | --- |
| Vertical line plus length 10 | `result=0`, `dof=2`, `failed=0` | Successful, intentionally underconstrained solve. |
| Same dimension repeated | `result=4`, `dof=2`, `failed=2 2 3` | `SLVS_RESULT_REDUNDANT_OKAY`; two related constraint handles reported. |
| Length 10 and incompatible length 20 | `result=1`, `dof=2`, `failed=2 2 4` | `SLVS_RESULT_INCONSISTENT`; related constraint handles reported. |
| 1,000 rebuild-and-solve repeats of first case | `stable=1`, `103.102 ms` total, `103.102 us` mean | Same result/DOF/no failed handles every time in this one container. |

The built shared object was 1,910,224 bytes and dynamically resolved `libatomic`, `libstdc++`, `libm`, `libgcc_s`, and glibc. This is a development-build observation, not a stripped release artifact size, cold-start measurement, or Linux compatibility commitment.

## Evidence and Interpretation

### SolveSpace `libslvs`

`slvs.h` is a public C interface: the caller supplies `Slvs_Param`, `Slvs_Entity`, and `Slvs_Constraint` arrays to `Slvs_Solve`; output includes `result`, unconstrained `dof`, and failed constraint handles. Its stateful convenience API also exposes `Slvs_SolveSketch`, and the current CMake target installs `libslvs` and `slvs.h`. This is a substantially narrower and more language-neutral seam than either PlaneGCS or FreeCAD.

The API is not a complete product boundary. It exposes numeric handles and mutable solver arrays rather than ThreeTerm sketch IDs, diagnostics, cancellation, or persistence. ThreeTerm must map every entity/constraint to stable IDs, copy solved coordinates only after success, normalize result codes, and make failed-handle calculation opt-in because upstream documents its roughly linear-in-constraint-count solve multiplier.

The fixture confirms the basic public diagnostic path, including handle return on a conflict. It does not establish that reported handles form a minimum explanation, nor does it cover tangent/equal/concentric/symmetric behavior, arcs/cubics, dragging, nonlinear convergence, or large sketches.

### PlaneGCS Extraction

Current FreeCAD source is clear evidence of solver capability but not of library maturity. `GCS::System` offers BFGS, Levenberg-Marquardt, and DogLeg algorithms, subsystem handling, solve/undo/apply operations, `diagnose()`, and queries for conflicting/redundant/partially redundant tags and dependent parameters. Its geometry and constraint headers include conics, B-splines, and related constraints that exceed `libslvs`' exposed entity set.

The same source shows why extraction is not a small reuse: `GCS.h` includes FreeCAD's `SketcherGlobal.h`; classes are exported as `SketcherExport`; and FreeCAD's CMake compiles `planegcs/*.cpp` into the `Sketcher` shared target linked to `Part` and `FreeCADApp`. There is no standalone CMake target, ABI versioning, packaging artifact, binding, or compatibility policy. A fork could be made technically, but would be a new maintained solver product rather than consumption of one.

### Existing FreeCAD/PlaneGCS Evidence

The prior rootless-Arch `FreeCADCmd` spike solved fully constrained rectangle and circle fixtures and observed redundant constraint detection. It also found that a redundant diagnostic arrived on solver output and that `SolverMessages` was not a stable scripting property in the tested version. That route provides rich functionality through the FreeCAD application, but its runtime and diagnostics must be normalized behind a worker protocol; it is not evidence for standalone PlaneGCS packaging.

### Commercial Candidates

D-Cubed 2D DCM, Spatial CDS, and C3D Solver may be viable evaluated alternatives, but no public primary material reviewed here establishes pricing, CI/redistribution rights, Linux artifact shape, cancellability, deterministic behavior, or executable test access. They cannot be compared honestly against the measured open-source fixture until the map's procurement/vendor gate provides access and terms.

## Required Validation Before Implementation

1. Select and document either GPLv3 distribution or a commercial `libslvs` license, including applicable notices and source obligations.
2. Run the `libslvs` shared adapter corpus: line/arc/circle tangent/equal/concentric/symmetric constraints; 50/200/1,000 constraints; repeated solve-after-edit; drag equivalents; redundancy/conflict/nonconvergence; and stable ThreeTerm ID mapping.
3. Put `libslvs` behind a killable worker and measure normal solve, deadline termination, restart, and no-partial-commit behavior. `libslvs` has no demonstrated cooperative path.
4. Repeat nonlinear and ambiguous cases across cold workers, dependency versions, and supported Linux targets. Define semantic and coordinate-tolerance determinism before using raw hashes.
5. Measure stripped/package size, cold/warm startup, RSS, and offline installation for the selected worker rather than extrapolating from the development library size.

## Primary Sources

- SolveSpace, [As a Library](https://solvespace.com/library.pl) (embedding caveat and GPL/commercial statement).
- SolveSpace v3.2, [`include/slvs.h`](https://github.com/solvespace/solvespace/blob/v3.2/include/slvs.h) (C ABI, entities, constraints, results, failed-handle cost) and [`src/slvs/CMakeLists.txt`](https://github.com/solvespace/solvespace/blob/v3.2/src/slvs/CMakeLists.txt) (shared target and installation).
- SolveSpace, [v3.2 release](https://github.com/solvespace/solvespace/releases/tag/v3.2) (released assets).
- FreeCAD, [`planegcs/GCS.h`](https://github.com/FreeCAD/FreeCAD/blob/552849c2855089130ed0c1cd86186edb1667b8b0/src/Mod/Sketcher/App/planegcs/GCS.h) and [`planegcs/Constraints.h`](https://github.com/FreeCAD/FreeCAD/blob/552849c2855089130ed0c1cd86186edb1667b8b0/src/Mod/Sketcher/App/planegcs/Constraints.h) (solver capabilities and LGPL SPDX headers).
- FreeCAD, [`Sketcher/App/CMakeLists.txt`](https://github.com/FreeCAD/FreeCAD/blob/552849c2855089130ed0c1cd86186edb1667b8b0/src/Mod/Sketcher/App/CMakeLists.txt) (PlaneGCS composition and FreeCAD target linkage).
- Existing local evidence: [`sketch-solver-spike.md`](./sketch-solver-spike.md) (FreeCAD headless fixture and diagnostic limitation).
