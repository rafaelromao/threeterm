# Persistent-Reference Stability Spike

Status: completed representative naming spike; not a kernel selection or a kernel-upgrade compatibility result.

## Question

Can candidate direct OCCT and headless FreeCAD routes preserve, or explicitly diagnose loss of, selected faces, edges, and vertices across representative history edits without silently selecting a different subshape?

## Method

The disposable corpus constructed a 40 x 30 x 20 box with two through-hole Boolean cuts. It retained four selections from the initial result:

- the left hole's cylindrical wall (face);
- the base top face (face);
- the base top-front edge (edge);
- the base top-origin corner (vertex).

It then rebuilt the feature history after a left-hole X-position edit, removed the left-hole Boolean feature, expanded a repeated-hole Boolean pattern from two to three members, applied a fillet to the selected edge, and persisted/reloaded a moved-hole model. The headless FreeCAD route saved and re-opened an FCStd document; direct OCCT wrote and read BREP.

For every case, the harness compared two persisted-reference candidates:

1. The original `Faces`/`Edges`/`Vertexes` traversal index.
2. A semantic descriptor containing feature provenance and role, plus the minimal geometric discriminator needed for the fixture, such as cylindrical-wall axis or top-face normal.

An index was `silently_wrong` when the selected semantic target no longer resolved but the old position was still occupied. A descriptor-only cylindrical-wall query deliberately omitted provenance to test ambiguity. Direct OCCT additionally asked `BRepFilletAPI_MakeFillet::Modified` for the selected source edge. This is a direct history hook, not an OCAF naming test.

## Environment And Assets

- Rootless Podman 6.0.1, `Host.Security.Rootless=true`; isolated disposable Arch Linux containers.
- Direct route: Arch `opencascade` 1:7.9.3-3, C++20.
- Application route: Arch `freecad` 1.1.3-1 / `FreeCADCmd` console mode.
- Source, build output, models, and raw result records: `/tmp/opencode/threeterm-reference-spike/`.
- Raw direct record SHA-256: `9396f57f47251b4ca69f39ff891d7a7a02098e3cc78b089bccdfde36d0421f23`.
- Raw FreeCAD record SHA-256: `fb45fa1ecb679c3e16c0afb5c1346067ae8c4ee1aa3b590e926908b9d5f84c31`.

The container installed the named packages, built the OCCT fixture with CMake, then executed the direct binary and `FreeCADCmd` fixture. Package installation required network access; the geometry programs did not access external input or application assets. No host geometry packages or production files were changed.

## Results

| Edit / reference | Direct OCCT | Headless FreeCAD | Interpretation |
| --- | --- | --- | --- |
| Move left-hole X, selected hole-wall face | Index correct; descriptor found | Index correct; descriptor found | One simple edit happened to retain traversal ordering. It is not an index-stability guarantee. |
| Move left-hole X, top face | Index correct; descriptor found | Index correct; descriptor found | Same limited observation. |
| Move left-hole X, top-front edge | Index correct; descriptor found | Index correct; descriptor found | Same limited observation. |
| Move left-hole X, top-origin vertex | Index correct; descriptor found | Index correct; descriptor found | Same limited observation. |
| Remove selected left-hole Boolean feature | Stale face index occupied; semantic descriptor not found | Same | A persisted traversal position would select an unrelated face instead of reporting deletion. |
| Expand hole pattern from two to three | Index correct; descriptor found | Index correct; descriptor found | The tested member retained its position; this does not cover insertion before a member, transforms, splits, or merges. |
| Cylindrical-wall query without provenance | Explicitly ambiguous (two matches) | Explicitly ambiguous (two matches) | Radius/shape alone cannot name symmetric pattern members. |
| Fillet selected top-front edge | Stale edge index occupied; `Modified(selectedEdge)` yielded no usable successor | Stale edge index occupied; Part-shape route exposes no history result | A consumed edge must diagnose loss or use feature-level provenance/history. Never fall back to the old index. |
| Reload moved-hole model | Index correct; descriptor found after BREP read | Index correct; descriptor found after FCStd reopen | Reload did not disturb this small same-version fixture; it does not make persisted subshape indices portable. |

Both route records contain nine evaluated references: six descriptor-resolved correct cases, one ambiguity, and two stale-index wrong cases. The two wrong cases are intentional adversarial history edits, not intermittent failures. The counts must not be generalized into a reliability percentage because the corpus is small and the selected cases are correlated.

## Findings

1. Raw topology traversal indexes are not safe persisted application references for either route. In both routes a deleted Boolean feature and a filleted edge left the stored location occupied by a different subshape, creating a silent misresolution unless the application independently validates identity.
2. A direct OCCT adapter must own stable application feature IDs and persisted semantic references. It may use `Generated`/`Modified`/`IsDeleted` as evidence during the same recomputation, but must record the input feature ID, reference role, operation generation, and enough geometry/adjoining context to validate its successor. The direct fillet result demonstrates that history may provide no successor for a consumed source edge; that condition must be explicit loss, not fallback selection.
3. A headless FreeCAD adapter may persist stable document-object IDs and feature dependencies, but `Part.Shape.Faces[n]`, `Edges[n]`, and `Vertexes[n]` have the same failure mode. This spike used Part-level shapes and did not demonstrate FreeCAD's higher-level topological naming across native PartDesign feature recomputation. FreeCAD therefore still requires ThreeTerm-level feature IDs and a diagnostic reattachment policy.
4. The minimum persisted application reference is not a kernel subshape index: `{schema_version, source_feature_id, source_output_role, optional_pattern_member_id, expected_topology_kind, geometric_predicates, adjacency/provenance_context, creation_generation}`. Reattachment returns exactly one of `resolved`, `ambiguous`, `lost`, or `incompatible`; it must never choose among zero or multiple candidates silently.
5. Explicit loss is useful state. The selected Boolean feature's disappearance should be reported as `lost` with the invalidating feature/revision, while the symmetric query should be `ambiguous` with candidates. Downstream features then follow the existing historical-edit policy: stop the affected branch, preserve last-valid geometry as stale, and offer correction, suppression, or recovery.

## Required Reattachment Policy

1. Recompute only from canonical feature definitions, never from serialized BREP/FCStd topology positions as authoritative input.
2. First use feature provenance and algorithm history within the recompute transaction. Validate expected kind, cardinality, role, and descriptor predicates against the candidate result.
3. If history is absent or non-unique, search only within the referenced feature output using the semantic descriptor and adjacency context. Exactly one validated candidate resolves; zero is `lost`; more than one is `ambiguous`.
4. Persist a diagnostic artifact containing old descriptor, candidate descriptors, native history/report data, kernel and feature-module versions, and the invalidating transaction.
5. Kernel upgrades are compatibility boundaries. Pin the worker/kernel version for normal replay; an upgrade must rebuild and run the complete naming corpus, then either migrate each reference with audited diagnostics or mark it incompatible. No transparent promise of cross-version topology continuity is supported by this evidence.

## Limitations

- The corpus has one geometry family and one run per route. It is evidence of failure modes, not a 100-combination reliability sweep.
- It did not test OCAF TNaming, FreeCAD native PartDesign naming/recomputation, CadQuery/OCP, kernel patch upgrades, tolerance changes, face splits/merges, near-tangent conditions, shell/draft/loft, circular transforms, or a persistent FreeCAD worker.
- The direct fillet query covers `Modified` only; it does not exhaust `Generated`, `IsDeleted`, Boolean history, or OCAF's label-based naming API.
- The BREP and FCStd reload checks occurred in the same Arch package versions and container environment. Cross-version, cross-distribution, and cross-platform persistence are unestablished.
- Semantic geometry predicates can also be ambiguous or tolerance-sensitive. They are a validation and fallback layer, not a substitute for feature provenance.

## Consequence

The persistent-reference gate is resolved only at the architectural-policy level: neither candidate may expose positional topology indexes as ThreeTerm persistence. Kernel selection still needs a broader naming torture corpus, including native OCAF and FreeCAD document-naming experiments, before either route can claim a topological-continuity advantage.
