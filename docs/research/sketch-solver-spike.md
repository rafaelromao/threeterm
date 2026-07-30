# Sketch Solver Spike

Status: initial candidate evidence; not solver selection.

## Environment

- Rootless Podman Arch Linux container
- FreeCAD 1.1.3 headless `FreeCADCmd`
- FreeCAD `Sketcher::SketchObject`, backed by FreeCAD's PlaneGCS implementation

## Fixture Results

| Fixture | Result |
| --- | --- |
| Rectangle | Four line segments with horizontal, vertical, coincident, width, height, and anchored-origin constraints returned `solve_result=0`, `FullyConstrained=True`, 4 geometry items, and 12 constraints. |
| Redundant horizontal constraint | Adding a duplicate horizontal constraint returned `solve_result=-2`; FreeCAD printed a redundant-constraint diagnostic identifying constraint 13. |
| Circle | Radius plus X/Y center constraints returned `solve_result=0`, `FullyConstrained=True`, one geometry item, and three constraints. |

## Findings

- FreeCAD headless exposes solve status and fully constrained state through its scripting surface.
- Redundant-constraint detection is observable without the GUI, but this version does not expose a `SolverMessages` property; diagnostics arrived through solver output.
- A stable ThreeTerm adapter cannot assume FreeCAD's UI-oriented properties or output streams are its public diagnostic contract.
- This fixture does not cover underconstraint DOF count, conflicting dimensions, tangent/equal/concentric/symmetric constraints, arcs, polylines, construction geometry, interactive dragging, solve-after-edit latency, deterministic branch stability, or standalone packaging.

## Gap

This is evidence for one application-embedded candidate, not a comparison with SolveSpace `libslvs`, an extracted PlaneGCS boundary, or commercial solvers. The FreeCAD/PlaneGCS route remains a possible option with application-runtime and API-surface cost; it is not selected.

## Asset

Disposable fixture: `/tmp/opencode/threeterm-geometry-spike/freecad_solver_spike.py`.

## Sources

- Solver options and risks: [`geometry-toolchain-options.md`](./geometry-toolchain-options.md)
- FreeCAD package: <https://archlinux.org/packages/extra/x86_64/freecad/>
