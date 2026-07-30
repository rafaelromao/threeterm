# Geometry Toolchain Options for ThreeTerm

- **Status:** Comparative research, not a technology decision
- **Researched:** 2026-07-30
- **Target:** Linux, terminal-native, deterministic parametric CAD for functional 3D printing

## Decision Statement

ThreeTerm needs a geometry stack that can support constrained 2D sketches, feature-based solid construction, mesh and CAD export, incremental recomputation, useful failure diagnostics, and safe cancellation. It must not implement a B-rep kernel, Boolean engine, tessellator, intersection engine, or sketch solver from scratch.

This note does **not** select a stack. It separates five decisions that should not be collapsed into one:

1. Geometry kernel.
2. Sketch constraint solver.
3. Host language and runtime boundary.
4. Plugin ABI or process protocol.
5. MVP feature and export scope.

The options are compared against primary sources. Claims are marked as follows:

- **Fact:** directly supported by a cited first-party source or source code.
- **Inference:** a conclusion from cited facts, but not a vendor guarantee.
- **Hypothesis:** an engineering proposition that needs a spike.
- **Source gap:** the reviewed primary sources do not establish the property.

Version and artifact-size observations are snapshots, not timeless properties.

## Executive Findings

1. **Fact:** Open CASCADE Technology (OCCT) provides the broad open-source B-rep baseline: solid and surface modeling, sweeps, Booleans, fillets, chamfers, hollowing, tessellation, validation, STEP, STL, shape healing, operation history hooks, and OCAF application-document services.[^occt-intro][^occt-history]
2. **Source gap:** OCCT does not document a general parametric sketch constraint solver. Its geometric construction routines and OCAF constraint attributes do not establish that capability. Kernel and sketch solver therefore remain separate choices.
3. **Fact:** CadQuery adds a productive, headless Python modeling layer over OCP/OCCT, including high-level holes and feature operations, but its constrained Sketch solver is explicitly experimental, supports only line segments and arcs in that mode, and its Sketch class has no history.[^cadquery-intro][^cadquery-sketch]
4. **Fact:** FreeCAD combines OCCT, a broad Python API, a parametric document model, and a substantial Sketcher solver. Its document API can recompute touched features and report per-object errors.[^freecad-readme][^freecad-document]
5. **Inference:** FreeCAD is closer to an application platform than a geometry library. This can remove feature-graph and solver work, but brings a much larger runtime, more application behavior to control, and a wider security and compatibility surface.
6. **Fact:** OCCT cancellation is cooperative through `Message_ProgressIndicator::UserBreak()` and progress ranges. This is useful only where the called algorithm propagates and checks the range.[^occt-progress]
7. **Hypothesis:** A disposable execution boundary, usually a killable worker process, is the only reviewed architecture that can guarantee prompt hard cancellation after cooperative cancellation fails. In-process C++, Python, or Rust cannot safely terminate an arbitrary stuck kernel call.
8. **Fact:** OCCT exposes generated/modified/deleted shape history, and OCAF provides topological naming machinery based on persistent labels plus algorithm history.[^occt-history][^occt-ocaf]
9. **Inference:** Operation history is necessary but not sufficient for product-grade persistent references. ThreeTerm must test semantic references across parameter changes, Boolean splits/merges, fillets, and kernel upgrades rather than equating `Generated()`/`Modified()` with a solved topological naming problem.
10. **Fact:** OCCT's standard exchange list includes STEP and STL but not 3MF. `lib3mf` is a first-party 3MF Consortium C++ reader/writer with C/C++ and other bindings, cross-platform binaries, and a BSD-2-Clause license.[^occt-intro][^lib3mf-readme][^lib3mf-license]
11. **Inference:** 3MF should be treated as a replaceable export component downstream of tessellation, not as a geometry-kernel selection criterion.
12. **Source gap:** None of the reviewed open-source candidates promises byte-identical geometry or export files across runs, thread counts, platforms, or versions. Determinism must be defined and measured at ThreeTerm's boundary.
13. **Fact:** Parasolid/D-Cubed, ACIS/CDS, and C3D publish broader integrated commercial kernel and solver offerings. Their public pages do not establish pricing, Linux redistribution terms, headless package size, cancellation semantics, or persistent-reference behavior for ThreeTerm.[^parasolid][^dcubed][^acis][^cds][^c3d]
14. **Fact:** CGAL and Manifold solve different problems. CGAL is a package-based computational geometry library with mixed LGPL/GPL licensing; Manifold is an Apache-2.0 library for reliable manifold triangle-mesh operations.[^cgal-license][^cgal-packages][^manifold-readme]
15. **Inference:** CGAL and Manifold can be useful companion components, but neither is a drop-in curved B-rep, sketch, feature-history, and STEP stack.

## Required Capability Baseline

The table records source-backed capability and explicitly marked gaps, not quality or fitness. Entries containing "verify" are not established capabilities.

| Capability | Direct OCCT | CadQuery/OCP | Headless FreeCAD | Commercial kernel plus solver | CGAL | Manifold |
|---|---|---|---|---|---|---|
| Constrained 2D sketch | No general solver documented | Experimental; constrained mode limited to segments/arcs | Documented product capability; PlaneGCS implementation present | Vendor products available | No integrated CAD sketch solver | No |
| Extrude and cut | Native prism and Boolean APIs | High-level APIs | Product feature and Python API | Advertised | Can compose selected polyhedral algorithms | Mesh Boolean composition |
| Holes | Compose primitive/tool plus cut | High-level hole, counterbore, countersink APIs | Product feature; verify exact headless API | Not verified from public vendor pages | Compose at mesh/polyhedral level | Compose mesh cut |
| Boolean union/intersection/difference | Native | High-level | Via OCCT/Part | Advertised | Package-dependent, including Nef polyhedra | Core focus |
| Fillet and chamfer | Native | High-level | Product feature | Advertised | Not a comparable B-rep feature | Not a curved B-rep feature |
| Revolve | Native | High-level | Product feature | Advertised | Not an integrated feature | Approximate through mesh generation only |
| Shell/thicken | Native offset/thick-solid APIs | High-level | Product feature | Advertised | Not an integrated feature | Not a curved B-rep feature |
| Patterns | Host-level repeated transforms/operations | High-level arrays and repeated operations | Product feature | Vendor-dependent | Host-level | Host-level |
| STEP | Native reader/writer | Documented import/export | Product capability; verify exact headless path | Vendor-dependent or add-on | No integrated CAD exchange | No |
| STL | Native tessellation/writer | Documented | Product capability; verify exact headless path | Vendor-dependent | Mesh I/O can be composed | Recommends other I/O packages |
| 3MF | Not in reviewed standard exchange list | Documented export | Verify exact headless path | Vendor-dependent | No integrated CAD exchange | Recommends 3MF but delegates I/O |
| Feature dependency graph | OCAF can be used, but application functions remain custom | No document graph; scripts rerun | Built-in document recomputation | Vendor-dependent | No | No |
| Operation history hooks | Generated/modified/deleted subshapes | Reachable through OCP and some CadQuery wrappers | Application-level naming and history | Vendor-dependent | Algorithm-specific | Original IDs/material provenance, not B-rep naming |

Primary capability sources: OCCT overview and modeling APIs,[^occt-intro] CadQuery introduction, Sketch, and API documentation,[^cadquery-intro][^cadquery-sketch] FreeCAD's product README and document source,[^freecad-readme][^freecad-document] CGAL package catalog,[^cgal-packages] and Manifold's project README.[^manifold-readme]

## Decision Decomposition

### Geometry Kernel

The kernel owns exact or tolerance-based geometry, topology, intersections, B-rep Booleans, local features, validation, healing, and tessellation. It should not own ThreeTerm's command language, user-facing feature IDs, or plugin ABI.

Kernel candidates:

| Candidate | Representation | Main integration route | Principal evidence-backed strength | Principal unresolved risk |
|---|---|---|---|---|
| OCCT | Curved B-rep plus triangulation | Native C++ or binding | Broad open CAD modeling and exchange coverage | Complexity, persistent references, cancellation coverage |
| Parasolid | Proprietary B-rep/facet/convergent model | Vendor C API/SDK | 900+ functions and broad commercial adoption advertised | Commercial and deployment terms |
| ACIS | Proprietary B-rep | Vendor C++ SDK | History/direct modeling, topology tracking, thread-safe APIs advertised | Commercial and deployment terms |
| C3D Modeler | Proprietary geometry kernel | C/C++, C#, TypeScript | Integrated toolkit and thread-safe kernel advertised | Commercial terms and independent validation |
| CGAL | Exact/robust computational geometry packages | C++ | Exact predicates/constructions and specialized algorithms | Not an integrated mechanical CAD stack; mixed licenses |
| Manifold | Manifold triangle mesh | C++, C, Python, JS/WASM, others | Reliable and parallel mesh Boolean focus | No exact curved B-rep or STEP feature model |

### Sketch Solver

The solver owns degrees of freedom, dimensional and geometric constraints, convergence, redundancy/conflict diagnosis, and ideally interactive dragging. It should not be inferred from the kernel choice.

Solver candidates:

| Candidate | Public integration surface | Diagnostics visible in source | License/terms | Main uncertainty |
|---|---|---|---|---|
| SolveSpace `libslvs` | C ABI in `slvs.h` | Result codes, DOF count, optional failed-constraint handles | GPLv3 or commercial | Upstream says solver is heavily coupled to the application |
| FreeCAD PlaneGCS | Internal C++ `GCS::System` | DOF, conflicting, redundant, partially redundant constraints; multiple algorithms | Source files marked LGPL-2.1-or-later | Not packaged or documented as a stable standalone SDK |
| CadQuery Sketch solver | Python API | Solve status available internally | CadQuery Apache-2.0; dependency audit still required | Explicitly experimental and limited geometry set |
| D-Cubed 2D DCM | Commercial component | Vendor advertises dimensional/geometric constraints and dragging | Proprietary | Price, Linux packaging, API access, redistribution |
| Spatial CDS | Commercial C++ API | Vendor advertises under/over-constraint analysis and diagnostics | Proprietary | Price, packaging, interoperability with chosen kernel |
| C3D Solver | Commercial module | Vendor advertises DOF detection and maintained constraints | Proprietary | Price, packaging, whether independently licensable |

### Host Language and Runtime Boundary

The host language affects integration effort, memory safety, package shape, and concurrency, but does not change the underlying geometry algorithms.

| Route | Facts | Inferences and hypotheses to test |
|---|---|---|
| Native C++ with OCCT | OCCT's primary API is C++; all modules are directly available.[^occt-intro] | Lowest wrapper skew and call overhead; highest exposure to OCCT API complexity and native faults |
| Python with CadQuery/OCP | CadQuery is headless and high-level; OCP aims for thin, broad OCCT bindings.[^cadquery-intro][^ocp-readme] | Fastest path to a feature prototype; runtime/GIL/process behavior must be measured |
| Python with pythonocc-core | Current project documentation advertises access to almost all OCCT classes and release 7.9.3.[^pythonocc-readme] | More direct OCCT surface than CadQuery; less product-level abstraction |
| FreeCAD command process | `MainCmd.cpp` initializes the non-GUI application and embedded Python runtime.[^freecad-cli] | High feature reuse and natural crash boundary; package and application lifecycle may dominate |
| Rust with `opencascade-rs` | Project describes itself as a major work in progress, uses `cxx`, bundles OCCT by default, and permits dynamic linking to a system OCCT.[^occt-rs] | Rust can protect ThreeTerm-owned code, not OCCT internals; wrapper coverage and API churn are gating risks |
| JavaScript/TypeScript with OCCT WASM | `opencascade.js` is an Emscripten port; its README identifies OCCT 7.6.2.[^occt-js] | Sandboxing is attractive, but version lag, memory, startup, module coverage, and native CLI fit need proof |
| Commercial native SDK | Vendors provide native SDKs, commonly C/C++ | Could reduce algorithm risk while increasing procurement and lock-in risk |

### Plugin ABI or Worker Protocol

This decision should be independent of the kernel and the ThreeTerm implementation language.

Candidate boundaries:

| Boundary | Isolation | ABI stability | Cancellation | Data cost | Main risk |
|---|---|---|---|---|---|
| Direct C++ API | None | Compiler/vendor dependent | Cooperative only | Lowest | Native crash or hang takes down host |
| Narrow in-process C ABI | None | Controllable if opaque handles and versioned structs are used | Cooperative only | Low | Lifetime, thread, and allocator mistakes remain process-fatal |
| Child process with versioned messages | Strong | Protocol can be language-neutral | Cooperative first, terminate process second | Serialization and startup | More lifecycle code and no shared kernel objects |
| WASM module boundary | Stronger memory sandbox | Explicit import/export ABI | Runtime interruption mechanism required | Copies/linear memory | OCCT WASM maturity and size |
| Embedded Python API | None unless Python is in a child process | Python package/version dependent | GIL and native-call behavior matter | Low in-process | Interpreter state and native extensions share failure domain |

**Hypothesis:** A versioned worker protocol should exchange ThreeTerm domain operations, stable feature IDs, structured diagnostics, and serialized shapes or meshes. It should not expose `TopoDS_Shape`, Python object identities, C++ standard-library types, or vendor object pointers.

**Hypothesis:** Even if the first implementation is in-process, defining this narrow protocol in the first spike prevents the geometry vendor API from becoming ThreeTerm's public plugin ABI.

### MVP Scope

Feature scope can invalidate an otherwise suitable stack. Compare at least two independent scope envelopes before selecting technology.

| Scope | Included | Deferred | What it tests |
|---|---|---|---|
| Printing-first core | Lines, arcs, circles; core constraints; additive/subtractive extrusion; straight and counterbored holes; fillet/chamfer; STL and 3MF | Revolve, shell, patterns, STEP authoring | Smallest credible functional-printing workflow |
| Interchange core | Printing-first core plus STEP export | Revolve, shell, patterns | Whether B-rep interoperability is an MVP requirement |
| Mechanical feature core | Interchange core plus revolve, shell, linear/circular patterns | Loft/sweep and assemblies | Broader kernel and persistent-reference stress |

**Hypothesis:** Shell and patterns add disproportionate naming and recomputation risk. They should not become kernel-selection requirements unless user evidence puts them in the MVP.

**Hypothesis:** Holes should initially be modeled as explicit semantic features whose implementation may be a cylinder/countersink cut. This preserves product intent without requiring a kernel-specific hole feature API.

## Option Details

### Direct OCCT from C++

**Facts**

- OCCT 8.0.0 documentation describes a modular C++ platform for B-rep modeling, algorithms, mesh, visualization, exchange, healing, and OCAF.[^occt-intro]
- The documented modeling layer includes prisms, revolutions, pipes, lofts, Boolean common/fuse/cut, hollowing, shelling, fillets, chamfers, and mechanical features.[^occt-intro]
- `BOPAlgo_Options` exposes errors, warnings, a report object, fuzzy tolerance, per-instance parallel mode, and a process-global parallel mode.[^occt-bop-options]
- `BRepCheck_Analyzer` checks topology and selected geometry conditions and can return subshape-specific results.[^occt-check]
- `BRepBuilderAPI_MakeShape` exposes `Generated`, `Modified`, and `IsDeleted`, and many concrete feature APIs override them.[^occt-history]
- OCAF supplies persistence, undo/redo, labels, dependency/function services, and topological naming based on algorithm history.[^occt-ocaf]
- The standard exchange list includes STEP, IGES, glTF, OBJ, VRML, and STL. It does not list 3MF.[^occt-intro]
- OCCT uses LGPL-2.1 plus an exception for header material in object code. Its own documentation calls out attribution, source availability, and relinking obligations for distribution.[^occt-license][^occt-exception]
- A complete OCCT installation is documented at approximately 2 GB, but core modules can be built without optional visualization dependencies.[^occt-intro]

**Inferences**

- This route provides maximum access to operation reports, progress ranges, history, and low-level validation.
- It also makes ThreeTerm responsible for the feature graph, solver integration, semantic error mapping, persistent references, packaging, and all unsafe-native containment.
- A reduced build should be substantially smaller than the complete documented installation, but that needs a reproducible package spike.

**Hypotheses to test**

- A narrow C++ worker can keep all OCCT objects private and expose only ThreeTerm's protocol.
- OCAF may remove enough graph, persistence, and naming work to justify its complexity, but a custom immutable feature graph may be smaller and easier to make deterministic.
- One worker thread or process per document, with internal OCCT parallel flags disabled by default, may be the simplest deterministic baseline.

### CadQuery and OCP

**Facts**

- CadQuery describes itself as a headless Python library for parametric 3D CAD over OCP/OCCT, with STEP, STL, AMF, and 3MF output.[^cadquery-intro]
- Its high-level API includes extrude, cuts, holes, fillet, chamfer, revolve, shell, sweep, loft, and array operations.[^cadquery-sketch]
- Its constrained Sketch mode is explicitly experimental, currently limited to segments and arcs, and supports a documented subset of constraints. The Sketch class does not implement history.[^cadquery-sketch]
- CadQuery's installation guide supports pip and conda, calls conda the better-tested and more mature route, and warns that bleeding-edge Python can lag its complex dependency set.[^cadquery-install]
- OCP says its goals are thin OCCT bindings, wrapping all practical modules, quick reaction to OCCT releases, and primary support for CadQuery.[^ocp-readme]
- The reviewed OCP/pywrap ordinary-method template uses direct pybind11 `.def(...)` calls without `py::gil_scoped_release` or `py::call_guard<py::gil_scoped_release>()`.[^ocp-template]
- As of the research date, PyPI reports `cadquery-ocp` 7.9.3.1.1. Its CPython 3.10-3.14 x86-64 Linux wheels are about 67.8 MB compressed; CadQuery 2.8.0's pure Python wheel is about 0.2 MB before dependencies.[^ocp-pypi][^cadquery-pypi]
- CadQuery and OCP are Apache-2.0, while the bundled or linked OCCT remains under its own LGPL terms.[^cadquery-license][^ocp-metadata][^occt-license]

**Inferences**

- Ordinary generated OCP calls should be treated as retaining the Python GIL until a runtime test proves otherwise. A Python thread is therefore not a reliable cancellation controller for a long generated native call.
- The released OCP wheel snapshot trails the reviewed OCCT 8.0.0 documentation. ThreeTerm must pin and test a matched wrapper/kernel tuple instead of combining independently "latest" packages.
- Python orchestration overhead may be negligible for expensive B-rep operations, but model traversal, serialization, and many tiny calls can change that result.
- CadQuery is a strong feature-prototyping surface, but its experimental sketch solver should not be silently promoted to ThreeTerm's production solver.

**Hypotheses to test**

- Running CadQuery/OCP in a child process can preserve development speed while solving hard cancellation and crash containment.
- ThreeTerm can bypass CadQuery selectors for persistent references and use explicit semantic references plus OCCT history through OCP.
- A minimal pip wheel environment may be easier to ship than conda despite conda's maturity; only a clean-container package spike can decide.

### pythonocc-core

**Facts**

- pythonocc-core 7.9.3 documents Python access to almost all OCCT classes, data exchange, visualization, conda packages for Linux and other platforms, and LGPL-3.0 licensing.[^pythonocc-readme]
- Its build configuration targets an exact OCCT 7.9.3 and can separately enable visualization, data exchange, and OCAF wrappers.[^pythonocc-cmake]
- The reviewed SWIG configuration does not enable SWIG's `-threads` option.[^pythonocc-cmake]

**Inferences**

- pythonocc-core is a direct binding option rather than a high-level feature model.
- The separately switchable modules can support a smaller build, but conda dependency and artifact sizes need measurement.
- As with OCP, long-call GIL and cancellation behavior must be demonstrated, not assumed.

### FreeCAD as a Headless Application Platform

**Facts**

- FreeCAD describes itself as a cross-platform parametric modeler with constrained 2D sketches, Python API, and OCCT as its geometry kernel.[^freecad-readme]
- `MainCmd.cpp` builds a non-GUI executable, initializes the application and embedded Python runtime, sets the numeric locale to `C`, runs the application, and closes documents during teardown.[^freecad-cli]
- `App::Document` tracks touched objects, dependencies, recomputing/error status, per-object error descriptions, transactions, and recomputation of all or a selected object subset.[^freecad-document]
- Current `App::Document` signals explicitly account for recomputation on a worker and marshal document signals to the main thread.[^freecad-document]
- PlaneGCS is part of FreeCAD's Sketcher source. It exposes BFGS, Levenberg-Marquardt, and DogLeg solvers plus DOF, conflicting, redundant, partially redundant, and dependent-parameter diagnostics. Its source is marked LGPL-2.1-or-later.[^freecad-gcs]
- FreeCAD's repository is identified as LGPL-2.1, but a shipped application contains dependencies with their own terms.[^freecad-metadata]
- The latest release snapshot was FreeCAD 1.1.3. Its official Linux x86-64 AppImage is 820,795,896 bytes, illustrating the full-application package envelope rather than a minimal headless build.[^freecad-release]

**Inferences**

- FreeCAD can supply more of the feature graph, recomputation, solver, and naming behavior than a kernel-only route.
- Its command process is naturally isolatable, but the host must still define deterministic document construction, startup state, add-on loading, preferences, logging, and error mapping.
- The full package is materially larger than a kernel-only or OCP worker. A custom headless build might be smaller but creates a downstream build-maintenance commitment.
- FreeCAD project files and scripting inputs should be treated as active content, not inert geometry. The 1.1.3 release itself contains security fixes for maliciously crafted files.[^freecad-release]

**Hypotheses to test**

- A persistent `FreeCADCmd` worker may amortize startup enough for interactive terminal use.
- Restricting the worker to ThreeTerm-generated Python and a private document format can reduce, but not eliminate, the application security surface.
- Reusing FreeCAD's naming behavior may outperform a small custom OCCT layer on parameter-edit survival, but the test must use ThreeTerm's feature semantics rather than FreeCAD GUI operations.

### Rust Binding Route

**Facts**

- `opencascade-rs` uses `cxx`, includes OCCT as a submodule by default, allows dynamic linking to an installed OCCT, and describes its API as a major work in progress and in flux.[^occt-rs]
- The repository license metadata is LGPL-2.1.[^occt-rs-metadata]

**Inferences**

- Rust can improve safety and clarity in ThreeTerm-owned orchestration but cannot make OCCT calls memory-safe or interrupt arbitrary native code.
- Bundling OCCT simplifies version pinning while increasing build time and artifact ownership. Dynamic linking reverses that tradeoff.
- Current wrapper coverage and churn are a larger selection risk than Rust itself.

### WebAssembly Route

**Facts**

- `opencascade.js` is an Emscripten port of OCCT to JavaScript/WebAssembly and advertises OCCT 7.6.2 in its README.[^occt-js]
- The repository's latest GitHub release is 1.1.1 from 2020, and repository metadata reports the last push in 2023.[^occt-js-release][^occt-js-metadata]
- OCCT 8.0.0's own requirements list Web builds using Emscripten SDK 3.0 or newer, so a current custom OCCT WASM build is a distinct option from the community binding.[^occt-intro]

**Inferences**

- The reviewed project is behind current OCCT and should not be assumed to cover the same APIs or fixes as OCCT 8.0.0.
- A current custom WASM build would replace binding-version risk with ownership of the build, binding surface, packaging, and upgrade work.
- WASM provides a useful memory boundary and language-neutral module shape, but Linux CLI startup, memory limits, module size, filesystem exchange, threading, and interruption need proof.
- This route is more compelling if browser execution becomes a requirement. It should not be selected merely to avoid defining a worker protocol.

### Commercial Kernels and Solvers

**Facts**

- Siemens advertises Parasolid as a 900+ function 3D modeler supporting B-rep, facet, surface/sheet, direct, and convergent modeling, licensed to more than 200 software vendors.[^parasolid]
- Siemens advertises D-Cubed 2D DCM as an embeddable 2D geometric constraint solver with dimensional/geometric constraints and dragging.[^dcubed]
- Spatial advertises ACIS as supporting direct and history-based modeling, Boolean/blend/thicken/offset operations, topology tracking, validation, thread safety, and multi-threaded APIs.[^acis]
- Spatial advertises CDS as a thread-safe C++ solver for 2D and 3D geometry with under/over-constraint analysis, diagnostics, dragging, and multiple solver modes.[^cds]
- C3D advertises an integrated toolkit with modeler, solver, converter, visualization, history, attributes, a thread-safe kernel, Linux support, and C/C++, C#, and TypeScript APIs.[^c3d]

**Source gaps**

- Public pages do not establish quote, royalty, minimum commitment, evaluation restrictions, source escrow, redistribution, or long-term support terms.
- Public pages do not establish ThreeTerm's required cancellation latency, deterministic output, process-global state, headless package size, or persistent-reference behavior.
- Parasolid's public product page reviewed here does not make a blanket thread-safety promise.

**Hypothesis:** Commercial evaluation should remain open until a fixed vendor questionnaire and the same black-box corpus can be run. Marketing breadth is not comparable to measured ThreeTerm behavior.

### CGAL and Manifold

**Facts**

- CGAL 6.2 organizes capabilities by package. Fundamental kernels are often LGPL, while many higher-level algorithms, including Nef polyhedra, are GPL; commercial licensing is available.[^cgal-license][^cgal-packages]
- CGAL's Nef polyhedra support general Boolean and topological operations over polyhedral sets, including non-manifold and mixed-dimensional geometry.[^cgal-packages]
- Manifold is dedicated to creating and operating on manifold triangle meshes, prioritizes reliable manifold output and performance, and provides optional TBB parallelization plus several language bindings.[^manifold-readme]
- Manifold is Apache-2.0.[^manifold-license]

**Inferences**

- CGAL's exact constructions are valuable for specialized computational geometry, but exact polyhedral Booleans do not supply mechanical B-rep features, a sketch solver, topological naming, or STEP.
- Manifold is attractive for mesh repair/Boolean/export validation paths, but choosing it as the primary kernel would redefine curves, fillets, STEP, and feature semantics as approximated mesh problems.
- Either can be a companion after the primary representation and interchange requirements are fixed.

## Sketch Solver Findings

### SolveSpace `libslvs`

**Facts**

- SolveSpace publishes a C ABI with parameter, entity, constraint, and system structs plus `Slvs_Solve` and convenience APIs.[^slvs-header]
- The public header includes points, workplanes, line segments, cubics, circles, arcs, and constraints including coincidence, distance, equality, symmetry, horizontal/vertical, diameter, angle, parallel, perpendicular, tangent, and dragged geometry.[^slvs-header]
- The result includes success/inconsistent/non-convergent/too-many-unknowns/redundant statuses, unconstrained DOF count, and optional failed-constraint handles. The header warns that finding failed constraints can cost about `n` solves for `n` constraints.[^slvs-header]
- SolveSpace states that its solver is heavily coupled to the rest of the application, but exposes the library for embedding.[^solvespace-library]
- SolveSpace is GPLv3 and offers commercial support/licensing.[^solvespace-library][^solvespace-license]

**Inferences**

- The C ABI is the cleanest open integration surface reviewed.
- GPLv3 can be a hard product gate depending on ThreeTerm's intended distribution license. A subprocess does not automatically answer the legal question; obtain legal review rather than designing around a guess.
- Coupling, diagnostics cost, handle lifecycle, numerical behavior, and thread safety need direct tests.

### FreeCAD PlaneGCS

**Facts**

- PlaneGCS exposes richer geometry and diagnosis in source than `libslvs`, including conics, B-splines, multiple nonlinear algorithms, subsystem partitioning, and redundant/conflicting constraint diagnosis.[^freecad-gcs]
- The public class is exported as part of FreeCAD's Sketcher module and includes FreeCAD-specific headers and build exports.[^freecad-gcs]

**Inferences**

- PlaneGCS is source-reusable in principle under its license, but it is not presented as a stable standalone package or ABI.
- Extracting it would create a maintained fork/build seam unless upstream adopts a standalone library boundary.
- Using it through FreeCAD avoids extraction but accepts the FreeCAD runtime.

### CadQuery Sketch Solver

**Facts**

- CadQuery's docs explicitly mark the constrained solver experimental, limit constrained geometry to line segments and arcs, and list a relatively small constraint set.[^cadquery-sketch]
- The current implementation is Python-owned and separate from OCCT's B-rep algorithms.[^cadquery-sketch-source]

**Inference:** It is suitable for a syntax and workflow prototype. Production suitability remains unestablished for conflict explanations, redundancy, large sketches, dragging, branch continuity, and long-term file compatibility.

## Cross-Cutting Risks

### Incremental Recomputation

**Facts**

- OCCT's `BRepMesh_IncrementalMesh` reuses correctly triangulated parts of a shape. That name concerns meshing and does not establish feature-graph recomputation.[^occt-mesh]
- OCAF provides function/dependency mechanisms for rebuilding objects after parameter changes, but application-specific modeling functions still have to be written.[^occt-ocaf]
- FreeCAD `App::Document::recompute()` checks touched objects, supports a selected subset, and uses dependency ordering.[^freecad-document]
- CadQuery Sketch explicitly has no history.[^cadquery-sketch]

**Hypothesis:** ThreeTerm should model recomputation as a deterministic DAG of immutable feature definitions and cached outputs unless the FreeCAD/OCAF spikes prove that adopting their document model is materially safer and smaller.

Required behavior:

1. A parameter edit invalidates only the changed feature and transitive dependents.
2. A failed feature preserves the last known-good upstream state and reports the blocked downstream chain.
3. Cache identity includes kernel/solver version, tolerances, feature schema, and all explicit inputs.
4. Cancellation never commits a partial feature result.

### Error Reporting and Debugging

**Facts**

- OCCT Boolean algorithms expose typed alerts, warnings, errors, and a `Message_Report`.[^occt-bop-options]
- OCCT shape validation can identify invalidity on specific subshapes.[^occt-check]
- FreeCAD exposes per-object error descriptions after recomputation.[^freecad-document]
- SolveSpace and PlaneGCS expose solver status and conflict/redundancy information, with different cost and detail.[^slvs-header][^freecad-gcs]

**Hypothesis:** The worker protocol should normalize all implementations into one diagnostic envelope:

```text
operation_id
feature_id
category
severity
user_message
technical_message
kernel_or_solver_code
related_feature_ids
related_constraint_ids
debug_artifact_paths
retryability
```

The spike must retain native reports and optional reproducer artifacts rather than reducing every failure to `operation failed`.

### Cancellation

| Route | Cooperative path | Hard-stop path | Evidence state |
|---|---|---|---|
| Direct OCCT in host | Progress indicator and `UserBreak()` where propagated | None safe in-process | Cooperative API documented |
| OCCT child worker | Same | Terminate worker and discard uncommitted output | Architecture hypothesis |
| OCP in host Python | OCCT progress types may be wrapped; ordinary generated calls appear to retain GIL | None safe in-process | Binding source inspection plus runtime test needed |
| Python child worker | Wrapper-dependent cooperative path | Terminate worker | Architecture hypothesis |
| FreeCAD command worker | Application-dependent | Terminate command process | Process exists; graceful cancellation needs spike |
| Commercial SDK in host | Vendor-dependent | None safe in-process | Vendor question required |
| Commercial child worker | Vendor-dependent | Terminate worker, subject to license/runtime behavior | Vendor question and spike required |

**Decision gate:** Any candidate that cannot either return from the cancellation corpus within the product latency budget or run inside a disposable worker fails the safety requirement.

### Concurrency and Global State

**Facts**

- OCCT provides explicit parallel switches for selected algorithms, and some switches are process-global, including Boolean and meshing defaults.[^occt-bop-options][^occt-mesh]
- OCCT's progress indicator supports concurrent processing, while documenting thread-safety obligations for callbacks.[^occt-progress]
- These APIs do not constitute a blanket guarantee that arbitrary OCCT objects can be mutated concurrently.
- Spatial explicitly advertises ACIS and CDS as thread-safe; C3D explicitly advertises a thread-safe kernel.[^acis][^cds][^c3d]
- Manifold can use TBB for selected algorithms but notes that not all work is parallel and its WASM build is serial by default.[^manifold-readme]

**Hypothesis:** Start with one active model operation per worker and no shared mutable kernel objects. Add concurrent documents or internal parallelism only after ThreadSanitizer/vendor-supported tests and determinism tests pass.

### Determinism

ThreeTerm must define several different guarantees:

| Level | Proposed meaning | Test |
|---|---|---|
| Semantic | Same inputs produce the same feature success/failure and user-visible references | Compare normalized feature result graph |
| Geometric | Same solid within explicit tolerance | Compare validity, volume, area, bounding box, topology counts, and bidirectional distance |
| Topological | Same persistent references resolve to the same intended semantic regions | Reference survival corpus |
| Mesh | Same tessellation parameters produce equivalent or identical indexed meshes | Normalize vertex/triangle ordering, then compare |
| File | Export bytes are identical | Hash raw and metadata-normalized files separately |

**Facts**

- FreeCAD's command entry point forces the numeric locale to `C`, removing one locale-dependent formatting variable.[^freecad-cli]
- OCCT exposes process-global and per-operation parallel controls.[^occt-bop-options][^occt-mesh]

**Source gap:** The reviewed documentation does not promise the five levels above across platforms or versions.

**Hypotheses to test**

- Disable internal parallelism for the deterministic baseline.
- Pin locale, units, tolerances, meshing parameters, dependency versions, CPU architecture target, and serialization metadata.
- Treat byte-stable STEP as a separate export requirement; geometric equivalence is not byte equality.
- Record implementation and dependency versions in every cache entry and project artifact.

### Persistent References and Topological Naming

**Facts**

- OCCT make-shape algorithms can report generated, modified, and deleted subshapes.[^occt-history]
- OCAF topological naming uses persistent reference keys, selected topology descriptions, and loaded operation history to recompute selections after topology changes.[^occt-ocaf]
- OCAF requires modeling algorithms to provide topology-evolution information.[^occt-ocaf]

**Inferences**

- Stable user-facing references need a ThreeTerm semantic layer even if OCAF or FreeCAD naming is adopted.
- Index-based references such as `edge[3]` are unsuitable as persisted product semantics.
- Geometric queries such as `highest Z face` are convenient but can become ambiguous after symmetry, patterns, or Boolean splits.
- A robust reference may need feature provenance, role, geometric predicates, adjacency context, and explicit ambiguity handling.

**Decision gate:** Run the same naming corpus through direct OCCT history, OCAF, CadQuery/OCP, and FreeCAD. A candidate fails if common parameter edits silently resolve to the wrong region. An explicit unresolved/ambiguous diagnostic is preferable to a wrong match.

### Packaging and Deployment

| Route | Verified snapshot | What still needs measurement |
|---|---|---|
| Complete OCCT | Docs estimate approximately 2 GB complete install; core can omit optional dependencies | Minimal stripped runtime, licenses, cold start, symbol/debug package |
| CadQuery/OCP | OCP Linux wheel about 68 MB compressed; CadQuery wheel about 0.2 MB before dependencies | Full environment, installed size, native library closure, startup, glibc floor |
| pythonocc-core | Conda packages documented for Linux | Full environment and minimal no-visualization build |
| FreeCAD | Official 1.1.3 x86-64 Linux AppImage 820,795,896 bytes | Persistent-worker startup, RSS, custom headless build feasibility |
| lib3mf | 2.5.0 Linux zip about 3.0 MB; Python Linux wheel about 1.6 MB | Installed size and integration with chosen mesh representation |
| opencascade-rs | Builds bundled OCCT by default or links installed OCCT | Build cache, artifact, coverage, distro compatibility |
| opencascade.js | No current package result established in this research | WASM/JS size, startup, memory ceiling, API coverage |
| Commercial | No public package result established | Redistributable closure, licensing daemon/files, offline behavior |

Artifact snapshots come from official release APIs and PyPI metadata.[^freecad-release][^lib3mf-release][^ocp-pypi][^cadquery-pypi]

### Licensing

This section records source facts, not legal advice.

| Component | Reviewed license fact | Product question |
|---|---|---|
| OCCT | LGPL-2.1 plus OCCT header exception | Dynamic/static distribution plan, notices, sources, relinking |
| CadQuery | Apache-2.0 | Audit transitive native dependencies and notices |
| OCP | Repository metadata Apache-2.0; wraps OCCT | Same OCCT obligations plus wrapper notices |
| pythonocc-core | LGPL-3.0 | Compatibility with ThreeTerm distribution model |
| FreeCAD | Repository metadata LGPL-2.1; bundled dependencies vary | Whole-distribution notices/source obligations and plugin licenses |
| PlaneGCS source | Files marked LGPL-2.1-or-later | Cost and obligations of extraction/fork |
| SolveSpace | GPLv3; commercial licensing offered | Whether ThreeTerm's intended license is compatible or a commercial grant is needed |
| CGAL | Package-specific LGPL-3+/GPL-3+; commercial option | Exact packages used and resulting obligations |
| Manifold | Apache-2.0 | Notices and optional dependency licenses |
| lib3mf | BSD-2-Clause; contains named third-party code | Bundle all notices and dependency licenses |
| Commercial SDKs | Proprietary | Evaluation, development, CI, redistribution, royalties, offline use, escrow |

## 3MF Export Path

**Facts**

- `lib3mf` provides 3MF reading, writing, conversion, and validation tools on Linux, Windows, and macOS, with bindings for multiple languages.[^lib3mf-readme]
- Version 2.5.0 publishes Linux packages, Python wheels, an SDK, and a WASM build.[^lib3mf-release]
- Its license is BSD-2-Clause, and its README identifies bundled third-party libraries whose notices must also be carried.[^lib3mf-license][^lib3mf-readme]

**Hypothesis:** The export seam should accept an indexed triangle mesh, units, object names, and optional material/color metadata, then write and re-open the 3MF with `lib3mf` validation. The source B-rep remains owned by the kernel worker.

**Spike cases**

1. Millimeter unit round-trip.
2. Multiple disconnected solids as separate objects and as one build item.
3. Degenerate and duplicate triangles.
4. Non-manifold input rejection.
5. Normals and winding.
6. Color/material preservation if included in MVP.
7. Deterministic ZIP metadata and entry ordering if byte-stable files are required.

## Comparative Spike Plan

All viable paths must run the same corpus and emit the same normalized result schema. A path-specific demo is not comparative evidence.

Do not begin by comparing only complete bundles. A FreeCAD-versus-CadQuery-versus-native result would conflate kernel access, solver, feature graph, language, and process architecture. Isolate the lanes first:

| Lane | Candidates | Fixed input/output boundary | Question isolated |
|---|---|---|---|
| Kernel | Direct OCCT C++; OCP/OCCT; FreeCAD Part; gated commercial kernels | Solved planar profiles and explicit feature operations in; normalized B-rep facts/history/diagnostics out | Geometry correctness, history, validation, performance |
| Solver | `libslvs`; PlaneGCS; CadQuery Sketch; gated commercial solvers | ThreeTerm sketch entities/constraints in; solved coordinates, DOF, conflicts, and status out | Solve behavior independent of B-rep kernel |
| Feature graph | Small ThreeTerm DAG; OCAF; FreeCAD Document | Identical feature definitions and edits in; invalidation/recompute trace out | Incremental behavior and transaction semantics |
| Runtime boundary | In-process narrow C ABI; child-process protocol; optional WASM | Same kernel adapter on each side | Startup, throughput, cancellation, crash containment |
| Exporter | OCCT STL/STEP; `lib3mf`; candidate application/vendor exporters | One validated B-rep and one canonical mesh in | Format validity, metadata, size, determinism |

Minimum open-source comparison set after the license gate:

1. A direct OCCT C++ worker.
2. A CadQuery/OCP Python worker using the same canonical solved profiles.
3. A FreeCAD command worker using its native Document and Sketcher path.
4. `libslvs`, PlaneGCS, and CadQuery Sketch tested as solver-only lanes where their licenses permit.
5. `lib3mf` used behind the same mesh-export interface for all kernel lanes.

Commercial products join the applicable lanes only after Spike H clears procurement and evaluation access.

### Spike A: Vertical Geometry Slice

Build a functional mounting bracket with:

1. A constrained sketch containing lines, arcs, circles, coincidence, horizontal/vertical, equal, tangent, dimensional, and fixed constraints.
2. Base extrusion.
3. Through pocket.
4. Two semantic holes, one counterbored.
5. Edge fillet and face/edge chamfer.
6. One parameter edit affecting sketch and all downstream features.
7. STL, 3MF, and STEP where supported.

Capture:

- Implementation lines outside generated code.
- Build and recompute latency, p50/p95.
- Peak RSS.
- Shape validity after every feature.
- Native diagnostics retained.
- Package size and cold startup.
- References used for each downstream selection.

### Spike B: Solver Behavior

Use the same sketch definitions with each solver:

1. Well-constrained sketch.
2. Under-constrained sketch with expected DOF.
3. Redundant but consistent sketch.
4. Conflicting dimensions.
5. Non-convergent initial guess.
6. Dragging a partially constrained point.
7. Sketches at 50, 200, and 1,000 constraints.

Pass conditions:

- No silent geometry corruption.
- DOF and conflict information maps to ThreeTerm constraint IDs.
- Failed solve leaves the last committed sketch unchanged.
- Repeated solves choose a stable branch or explicitly report ambiguity.
- Interactive-size cases satisfy the product latency budget.

### Spike C: Cancellation and Crash Containment

Create expensive Boolean, fillet, shell, tessellation, and solver cases. Request cancellation at randomized points.

Measure:

- Cooperative-cancel latency.
- Hard-stop latency.
- Whether the terminal host remains responsive.
- Whether partial output is ever committed.
- Whether the worker can restart and recompute from persisted inputs.
- Behavior on native exception, segmentation fault, out-of-memory, and infinite/very long operation.

Pass conditions:

- The host survives every injected worker failure.
- Cancellation never publishes a partial feature result.
- A worker that misses the cooperative deadline can be terminated and replaced.
- Recovery requires only immutable inputs and committed cache artifacts.

### Spike D: Persistent Reference Torture

Persist references to a filleted edge, a hole wall, the top face, and a Boolean-generated edge. Sweep dimensions through at least 100 parameter combinations that trigger:

1. Edge reorder.
2. Face split.
3. Face merge.
4. Feature disappearance and reappearance.
5. Symmetry/ambiguity.
6. Near-tangent Boolean conditions.
7. Kernel patch-version upgrade.

Classify every result as correct, explicitly unresolved, explicitly ambiguous, or silently wrong.

Pass condition: zero silently wrong resolutions in the corpus. Correctness rate and unresolved rate remain separate metrics.

### Spike E: Determinism

For every vertical-slice model:

1. Run 20 cold and 20 warm recomputations.
2. Run with internal parallelism off and on.
3. Run in separate processes.
4. Run on two supported Linux distributions and CPU vendors.
5. Repeat after serialization/reload.

Compare semantic graph, validation, mass properties, topology, normalized B-rep/mesh, raw export bytes, and metadata-normalized export bytes.

Pass conditions must be set independently for semantic, geometric, topological, mesh, and file determinism. Do not use raw file hash as the sole geometry oracle.

### Spike F: Packaging

Produce a clean-container artifact for each viable route and record:

- Compressed and installed size.
- Complete dynamic-library closure.
- Minimum glibc and CPU requirements.
- Cold and warm startup.
- Idle and active RSS.
- Offline installation.
- Reproducible build steps and build time.
- License and source-offer payload.
- Debug-symbol and crash-dump strategy.

### Spike G: Boundary Prototype

Implement one versioned protocol with:

- `hello` and capability negotiation.
- `open_model` from immutable feature definitions.
- `recompute` with operation ID.
- `cancel`.
- `export_mesh`, `export_step`, and `export_3mf` capability responses.
- Structured diagnostics.
- Progress events.
- Crash/restart recovery.
- No vendor object handles in messages.

Run at least one C++ OCCT worker and one Python/FreeCAD or Python/CadQuery worker behind the same client. The purpose is to test boundary cost and substitutability, not to choose the implementation.

### Spike H: Commercial Vendor Gate

Send every vendor the same questionnaire before spending evaluation engineering time:

1. Linux x86-64 and arm64 support.
2. Headless redistribution and offline operation.
3. Development, CI, seat, royalty, and minimum fees.
4. Exact APIs for required features and STEP/STL/3MF.
5. Constraint solver availability independent of kernel.
6. Thread-safety model and process-global state.
7. Cooperative cancellation and progress callbacks per operation.
8. Persistent topology/reference APIs and documented limitations.
9. Determinism guarantees.
10. Package size and supported compiler/ABI matrix.
11. Evaluation access to headers, docs, examples, and benchmark rights.
12. Source escrow, end-of-life, and patch policy.

Only vendors that clear legal, budget, Linux, and evaluation-access gates enter the technical corpus.

## Decision Gates

No weighted score should be produced until these gates are resolved:

1. **License gate:** Fix ThreeTerm's intended distribution license and whether GPL components or commercial licenses are acceptable.
2. **Scope gate:** Choose which of the three MVP envelopes is real.
3. **STEP gate:** Decide whether STEP is required at launch or is a later interchange feature.
4. **Solver gate:** Require measured conflict diagnostics, branch stability, and target sketch scale.
5. **Safety gate:** Require disposable process isolation or demonstrated cooperative cancellation for every long operation.
6. **Naming gate:** Require zero silent misresolution in the naming corpus.
7. **Determinism gate:** Define semantic, geometric, topological, mesh, and file guarantees separately.
8. **Packaging gate:** Set maximum artifact, startup, memory, distro, and offline-install budgets.
9. **Vendor gate:** Set maximum recurring cost, lock-in, and procurement lead time.
10. **Maintenance gate:** Decide whether ThreeTerm can own generated bindings, a PlaneGCS extraction, a custom FreeCAD build, or a kernel fork.

The gates intentionally permit mixed outcomes. For example, the kernel, solver, 3MF writer, host language, and isolation boundary can come from different options.

## Open Questions

1. What license will ThreeTerm use, and will proprietary distribution ever be required?
2. Is STEP export launch-critical, or are STL and 3MF sufficient for the first release?
3. Is interactive sketch dragging required in a terminal MVP, or only solve-after-edit?
4. What is the largest expected sketch and feature graph?
5. What cancellation latency is acceptable before a worker is killed?
6. Is byte-identical export required, or is deterministic geometry plus normalized metadata sufficient?
7. Must projects survive kernel upgrades with all references intact, or may an upgrade perform an explicit migration/rebuild?
8. What are the compressed artifact, installed size, startup, and RSS budgets?
9. Are user-authored plugins in-process, out-of-process, or not part of MVP?
10. Can the product ship an embedded Python or full FreeCAD runtime?

## Source Register

All sources below are first-party project documentation, source code, package metadata, specifications, or vendor product pages. Accessed 2026-07-30.

[^occt-intro]: Open CASCADE Technology 8.0.0, [Introduction and module overview](https://dev.opencascade.org/doc/overview/html/index.html).
[^occt-bop-options]: Open CASCADE Technology 8.0.0, [`BOPAlgo_Options` reference](https://dev.opencascade.org/doc/refman/html/class_b_o_p_algo___options.html).
[^occt-progress]: Open CASCADE Technology 8.0.0, [`Message_ProgressIndicator` reference](https://dev.opencascade.org/doc/refman/html/class_message___progress_indicator.html).
[^occt-check]: Open CASCADE Technology 8.0.0, [`BRepCheck_Analyzer` reference](https://dev.opencascade.org/doc/refman/html/class_b_rep_check___analyzer.html).
[^occt-history]: Open CASCADE Technology 8.0.0, [`BRepBuilderAPI_MakeShape` reference](https://dev.opencascade.org/doc/refman/html/class_b_rep_builder_a_p_i___make_shape.html).
[^occt-mesh]: Open CASCADE Technology 8.0.0, [`BRepMesh_IncrementalMesh` reference](https://dev.opencascade.org/doc/refman/html/class_b_rep_mesh___incremental_mesh.html).
[^occt-ocaf]: Open CASCADE Technology 8.0.0, [OCAF user guide](https://dev.opencascade.org/doc/overview/html/occt_user_guides__ocaf.html).
[^occt-license]: Open CASCADE Technology, [licensing page](https://dev.opencascade.org/resources/licensing).
[^occt-exception]: OCCT source, [`OCCT_LGPL_EXCEPTION.txt`](https://raw.githubusercontent.com/Open-Cascade-SAS/OCCT/master/OCCT_LGPL_EXCEPTION.txt).
[^cadquery-intro]: CadQuery, [Introduction](https://cadquery.readthedocs.io/en/latest/intro.html).
[^cadquery-sketch]: CadQuery, [Sketch documentation](https://cadquery.readthedocs.io/en/latest/sketch.html).
[^cadquery-install]: CadQuery, [Installation documentation](https://cadquery.readthedocs.io/en/latest/installation.html).
[^cadquery-license]: CadQuery source, [LICENSE](https://raw.githubusercontent.com/CadQuery/cadquery/master/LICENSE).
[^cadquery-pypi]: PyPI, [`cadquery` JSON metadata](https://pypi.org/pypi/cadquery/json).
[^cadquery-sketch-source]: CadQuery source, [`cadquery/sketch.py`](https://raw.githubusercontent.com/CadQuery/cadquery/master/cadquery/sketch.py).
[^ocp-readme]: CadQuery OCP source, [README](https://raw.githubusercontent.com/CadQuery/OCP/master/README.md).
[^ocp-template]: CadQuery pywrap source, [`template_sub.j2`](https://raw.githubusercontent.com/CadQuery/pywrap/master/bindgen/template_sub.j2).
[^ocp-pypi]: PyPI, [`cadquery-ocp` JSON metadata](https://pypi.org/pypi/cadquery-ocp/json).
[^ocp-metadata]: GitHub API, [CadQuery/OCP repository metadata](https://api.github.com/repos/CadQuery/OCP).
[^pythonocc-readme]: pythonocc-core source, [README](https://raw.githubusercontent.com/tpaviot/pythonocc-core/master/README.md).
[^pythonocc-cmake]: pythonocc-core source, [`CMakeLists.txt`](https://raw.githubusercontent.com/tpaviot/pythonocc-core/master/CMakeLists.txt).
[^freecad-readme]: FreeCAD source, [README](https://raw.githubusercontent.com/FreeCAD/FreeCAD/main/README.md).
[^freecad-document]: FreeCAD source, [`src/App/Document.h`](https://raw.githubusercontent.com/FreeCAD/FreeCAD/main/src/App/Document.h).
[^freecad-cli]: FreeCAD source, [`src/Main/MainCmd.cpp`](https://raw.githubusercontent.com/FreeCAD/FreeCAD/main/src/Main/MainCmd.cpp).
[^freecad-gcs]: FreeCAD source, [`src/Mod/Sketcher/App/planegcs/GCS.h`](https://raw.githubusercontent.com/FreeCAD/FreeCAD/main/src/Mod/Sketcher/App/planegcs/GCS.h).
[^freecad-metadata]: GitHub API, [FreeCAD repository metadata](https://api.github.com/repos/FreeCAD/FreeCAD).
[^freecad-release]: GitHub API, [FreeCAD latest release metadata](https://api.github.com/repos/FreeCAD/FreeCAD/releases/latest).
[^solvespace-library]: SolveSpace, [As a Library](https://solvespace.com/library.pl).
[^slvs-header]: SolveSpace source, [`include/slvs.h`](https://raw.githubusercontent.com/solvespace/solvespace/master/include/slvs.h).
[^solvespace-license]: SolveSpace source, [`COPYING.txt`](https://raw.githubusercontent.com/solvespace/solvespace/master/COPYING.txt).
[^lib3mf-readme]: 3MF Consortium, [lib3mf repository and README](https://github.com/3MFConsortium/lib3mf).
[^lib3mf-license]: lib3mf source, [LICENSE](https://raw.githubusercontent.com/3MFConsortium/lib3mf/master/LICENSE).
[^lib3mf-release]: GitHub API, [lib3mf latest release metadata](https://api.github.com/repos/3MFConsortium/lib3mf/releases/latest).
[^occt-rs]: opencascade-rs source, [README](https://raw.githubusercontent.com/bschwind/opencascade-rs/main/README.md).
[^occt-rs-metadata]: GitHub API, [opencascade-rs repository metadata](https://api.github.com/repos/bschwind/opencascade-rs).
[^occt-js]: opencascade.js source, [README](https://raw.githubusercontent.com/donalffons/opencascade.js/master/README.md).
[^occt-js-release]: GitHub API, [opencascade.js latest release metadata](https://api.github.com/repos/donalffons/opencascade.js/releases/latest).
[^occt-js-metadata]: GitHub API, [opencascade.js repository metadata](https://api.github.com/repos/donalffons/opencascade.js).
[^cgal-license]: CGAL, [license overview](https://www.cgal.org/license.html).
[^cgal-packages]: CGAL 6.2, [package overview](https://doc.cgal.org/latest/Manual/packages.html).
[^manifold-readme]: Manifold source, [README](https://raw.githubusercontent.com/elalish/manifold/master/README.md).
[^manifold-license]: Manifold source, [LICENSE](https://raw.githubusercontent.com/elalish/manifold/master/LICENSE).
[^parasolid]: Siemens, [Parasolid](https://www.siemens.com/en-us/products/plm-components/parasolid/).
[^dcubed]: Siemens, [D-Cubed 2D DCM](https://www.siemens.com/en-us/products/plm-components/d-cubed/2d-dcm/).
[^acis]: Spatial, [3D ACIS Modeler](https://www.spatial.com/solutions/3d-modeling/3d-acis-modeler).
[^cds]: Spatial, [Constraint Design Solver](https://www.spatial.com/solutions/3d-modeling/constraint-design-solver).
[^c3d]: C3D Labs, [C3D Toolkit](https://www.c3dlabs.com/en/products/c3d-toolkit/).
