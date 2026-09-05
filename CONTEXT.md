# ThreeTerm

ThreeTerm is a Linux terminal-native parametric CAD product for designing functional parts for 3D printing. Release-namespace review remains required before public distribution.

## Language

**ThreeTerm**:
The product and CLI name for this terminal-native parametric CAD application.

**Named Revision**:
A recoverable prior future preserved when new work diverges from an undone active timeline. It is not a general-purpose branch.
_Avoid_: Branch, fork

**Geometric Kernel**:
The version-pinned OCCT worker-owned engine that constructs, validates, tessellates, and exchanges ThreeTerm solid geometry. It does not own ThreeTerm feature identity, persistent-reference semantics, or authoritative document state.

**Revision Snapshot**:
The immutable, host-owned canonical feature graph and deterministic transaction inputs at one document revision. It is the only authority for recomputation eligibility and replay.

**Canonical Transaction Log**:
The ordered, sealed durable record of accepted versioned command transactions, named-revision metadata, and durable recovery provenance. Together with its manifest, it is the source from which a document's canonical state is recovered; it contains no worker artifacts or transient UI state.

**Persisted Checkpoint**:
A serialized materialization of a Revision Snapshot at a declared Canonical Transaction Log position. It accelerates opening and may be discarded and rebuilt after integrity or compatibility failure; it does not compete with the Canonical Transaction Log as authority.

**Project Manifest**:
The sealed compatibility and integrity metadata that identifies the project/container schemas, canonical-log position, and required command, feature, worker, kernel, and solver versions. It controls whether canonical data may be interpreted or replayed.

**Project Generation**:
One complete sealed on-disk save of a project bundle. The current and immediately preceding valid generations provide bounded recovery; generations do not make caches authoritative.

**Project Identity**:
The externally inspectable identity of one loaded Project Generation: its generation digest, Revision Snapshot ID and hashes, and Canonical Transaction Log position.

**Derived Result**:
A revision-bound, non-authoritative worker result, cache entry, or staged artifact. It becomes current only after host validation and atomic promotion against its Revision Snapshot.

**Stale Last-Valid Geometry**:
The explicitly non-authoritative Derived Result retained after a historical edit invalidates a feature. It is inspectable for recovery but cannot represent the edited document as current or silently pass validation or export.

**Official Interactive Environment**:
The named terminal emulator version, direct local attachment topology, and validated capability vector for which ThreeTerm promises its graphical interactive MVP. It is narrower than a terminal brand or a `$TERM` value.

**Terminal Capability Vector**:
The attachment-scoped set of positively probed graphics, input, presentation, and path capabilities used to decide whether the Official Interactive Environment may start. Environment variables and terminal names are diagnostic hints, not capability evidence.

**Protocol-Neutral Viewport**:
The host-owned projection boundary that turns an immutable scene/camera/selection state into a disposable viewport frame without depending on a terminal graphics protocol. It does not own CAD truth, picking authority, or command state.

**Viewport Frame**:
One complete, revision- and size-bound raster presentation of the current Protocol-Neutral Viewport state, including selection and transient visual feedback. It is disposable and may be coalesced or dropped before terminal submission.

**Active Viewport Image**:
The one attachment-scoped Kitty image and placement that displays the current Viewport Frame in the Official Interactive Environment. It has no durable identity and must be explicitly deleted during cleanup.

**Headless Automation**:
CLI or MCP invocation of the versioned domain command API without a terminal viewport, focus, selection, camera, pointer gesture, or other interactive presentation state. It can inspect, validate, export, preview, and atomically commit model commands using explicit semantic inputs.

**Interactive Modeling**:
The graphical, direct-manipulation workflow that requires the Official Interactive Environment and its positively probed Terminal Capability Vector. It is distinct from Headless Automation and is unavailable when that capability gate fails.

**Command Draft**:
An uncommitted set of explicit semantic command inputs being collected by an interactive caller. It is presentation state, not a Canonical Transaction Log entry or a model mutation.

**Command Preview**:
A cancellable, revision-bound read-only evaluation of a Command Draft. It may expose transient derived results and diagnostics but never changes canonical state or creates a transaction.

**Transient Interaction State**:
The non-durable focus, selection, gesture, command, preview, and recovery status of one Interactive Modeling session. It may affect presentation and input routing but cannot become model truth without the shared command API's validated atomic commit.

**Gesture Acknowledgement**:
A visible indication of a pending target, active tool or drag, selected result, cancellation, focus/resize recovery, or readiness. It makes transient input state observable without requiring hover, release, or pixel-coordinate events for correctness.

**Sketch**:
An immutable host-owned collection of stable-ID sketch entities and constraints. Its
solver output is a Derived Result until a successful revision-bound command commits
the resolved coordinates.

**Sketch Entity**:
A stable-ID point, line segment, circle, or arc in a Sketch. Entity IDs are the
ThreeTerm identity carried across the libslvs worker boundary; solver handles are
never canonical.

**Sketch Solve**:
A versioned command that evaluates a Sketch through a disposable libslvs worker and
normalizes status, degrees of freedom, entity IDs, related constraints, diagnostics,
and successful solved coordinates.

**Fit Dimension**:
A revision-bound relationship between stable sketch dimension constraints associated with a source solid and a target solid. The host derives both values from canonical sketches and validates the target against the source clearance before persisting the relationship.

**Extrusion Mode**:
The versioned domain input that selects additive prism creation or subtractive prism cutting. The mode is canonical intent, not worker state.

**Semantic Extrusion Target**:
The stable feature identity of the canonical solid consumed by a subtractive extrusion. The host resolves and authenticates its disposable BREP from canonical provenance; worker paths are never persisted as the target.

**Planar-Face Support**:
The semantic reference to one planar face in a canonical solid. It records stable feature and face provenance, semantic role, planar geometric evidence, and an explicit reattachment outcome; it never records a topology index or native worker handle.

**Sketch Placement**:
A revision-bound right-handed orthonormal frame mapping local sketch coordinates to world coordinates. Its origin and axes are durable intent and are validated against the selected planar-face frame before solving or rendering.
