# State, History, and Extensibility Options

- Status: research note, not an architecture decision
- Date: 2026-07-30
- Scope: ThreeTerm's state/history model, project persistence, extension boundaries, and shared command surface

## Purpose

ThreeTerm's vision combines parametric modeling, complete and understandable history, historical editing, deterministic replay, undo/redo, eventual branches and semantic merge, fast startup, a TUI and headless interface, plugins, and Lua configuration. Those requirements interact, but they do not all describe the same mechanism.

This note compares the main options and identifies the evidence needed before selecting one. It deliberately does not choose a final architecture, storage technology, project container, plugin runtime, or public API.

### Evidence labels

- **Fact**: behavior documented by a primary source, specification, or mature system's official documentation. Citations use `[S#]` and are collected in [Sources](#sources).
- **Hypothesis**: a ThreeTerm-specific inference that still needs validation.
- **Decision**: a product or architecture choice that the available evidence cannot make for ThreeTerm.
- **Constraint**: a requirement from the supplied ThreeTerm vision, not an externally verified fact.

## Executive Summary

1. **Fact:** A CAD feature/dependency graph, a chronological command or event history, and a revision DAG solve different problems. A system may need all three, one, or two; choosing a DAG for one does not imply a DAG for another.[S1][S3][S9]
2. **Fact:** Pure event sourcing does not remove the need for a feature graph. It makes that graph a projection rebuilt from events. The substantive choice is which representation is canonical and what must be versioned forever.[S1][S3]
3. **Fact:** An immutable event log alone does not guarantee deterministic replay. Replay also depends on deterministic handlers, stable schemas and algorithms, captured external inputs, execution ordering, dependencies, and version policy. Temporal treats workflow code changes as a replay compatibility problem, while Wasmtime exposes explicit controls for deterministic WebAssembly execution.[S4][S18]
4. **Fact:** Mature CAD infrastructure such as OCCT's OCAF combines a document model, transaction deltas for undo/redo, a function dependency graph for regeneration, and persistence drivers. This is evidence that those concerns can be separate; it is not evidence that the same split is right for ThreeTerm.[S1]
5. **Fact:** A revision DAG provides ancestry and branch structure, not semantic CAD merge by itself. Semantic diff and merge additionally require stable feature/object identity, schema-aware comparison, conflict rules, and robust references across topology changes. Onshape's merge rules operate at document/tab/feature semantics and still expose incompatible changes and merge failures.[S7][S8]
6. **Fact:** Topological naming is independent of event sourcing. OCCT requires modeling operations to report generated, modified, and deleted topology and documents cases where naming cannot resolve ambiguity.[S1]
7. **Fact:** Source-code module boundaries and runtime plugin boundaries are independent. Static modules can preserve internal seams without creating a public ABI, process protocol, sandbox, or plugin lifecycle.[S15]
8. **Fact:** Native plugins, WebAssembly, out-of-process extensions, and Lua have materially different trust, failure, versioning, startup, and call-granularity properties. No one mechanism is a neutral implementation detail.[S12][S16][S20][S22]
9. **Hypothesis:** The most consequential extension decision is not the runtime. It is whether third-party code may define persistent parametric features. If it may, project opening, replay, migration, topological naming, headless execution, diff/merge, and long-term compatibility all cross the plugin boundary.
10. **Decision:** The phrases "complete parametric history," "edit a historical operation," "deterministic replay," "branches," and "plugins" need narrower product semantics before implementation machinery can be justified.

## Separate the State Models

The word "history" currently covers at least three structures:

| Structure | Nodes represent | Edges/order represent | Primary purpose |
| --- | --- | --- | --- |
| Feature model | Parametric features and durable model objects | Feature order and/or dependencies | Regeneration and user-authored model intent |
| Operation history | Accepted commands, domain events, or transaction deltas | Chronology and causality | Audit, undo/redo, replay, and object-focused timelines |
| Revision history | Saved or named model revisions | Parentage and merge ancestry | Versions, branches, comparison, and collaboration |

They can be related without being identical:

```text
TUI / headless / Lua
        |
        v
  command invocation ------> chronological command/event records
        |                                  |
        v                                  v
 feature model ------dependency------> recomputation
        |                                  |
        +-------------> project snapshot <-+
                               |
                               v
                     revision history graph
```

### Terms used in this note

- **Command:** a request to perform an operation. It can be rejected because its arguments, permissions, or current context are invalid.
- **Domain event:** a durable statement that an accepted domain change occurred. It is not a request and should not represent failed intent.
- **Transaction delta:** enough before/after state to reverse or reapply a committed state mutation. It need not contain user intent or be replayable against a different base state.
- **Feature graph:** durable parametric entities plus the dependencies needed to identify and recompute affected results. A UI may present a total feature order even if execution dependencies form a DAG.
- **Revision:** an immutable identity for a coherent project state, whether represented by a snapshot, a log position, or both.
- **Snapshot:** a state image associated with a revision or log position. A snapshot may be canonical state, a replay checkpoint, or a disposable cache; these meanings must not be conflated.
- **Derived geometry:** kernel data such as B-reps or meshes that can in principle be recomputed from parametric inputs, subject to compatible algorithms and dependencies.
- **Topological reference:** a reference from durable model intent to a face, edge, vertex, or other subshape that may change identity when upstream geometry changes.

### The feature list is not necessarily the dependency graph

**Fact:** Onshape presents a sequential feature list and regenerates from the feature before the rollback bar through later features. Features can also have explicit references and dependencies.[S2] **Fact:** OCAF's function mechanism records dependencies and executes affected functions in dependency order.[S1]

**Hypothesis:** ThreeTerm may want an ordered authoring narrative for comprehension while using a dependency graph to limit recomputation. Treating the visible timeline and execution graph as the same structure could either serialize independent work unnecessarily or make the user-facing history harder to understand.

**Decision:** Decide whether feature order is semantically meaningful, merely a default presentation, or both. This affects historical edits, diff, merge, and whether independent features may be reordered.

## State and History Options

### Option A: Canonical feature graph plus command journal

In this family, the saved feature/object graph is the canonical model. Accepted commands mutate it through transactions. A journal may be transient undo data, a durable audit trail, a replay source, or some combination. Those variants have very different obligations.

```text
command -> validate -> transaction -> canonical feature graph
                              |              |
                              v              v
                       journal/delta    affected recomputation
```

#### Evidence

- **Fact:** OCAF stores application data in a document, records modifications as transaction deltas, supports undo/redo over those deltas, and separately uses function dependencies to update affected results.[S1]
- **Fact:** Qt's undo framework models user actions as commands. A new command after undo deletes the redo tail in the default linear stack, and command merging or macros can change user-visible undo granularity.[S5]
- **Fact:** Blender operators are reusable actions invoked by UI controls, keymaps, search, and Python, but their success can depend on an implicit context and a `poll` check.[S10]

#### Advantages

- The current parametric state is directly available after loading the canonical graph; full journal replay is not inherently required for ordinary startup.
- Feature dependencies and dirty-state propagation can be modeled explicitly around the primary CAD workload.
- Transaction deltas can make local undo/redo independent of long-term event schema compatibility.
- The journal can record user-level intent for an object-focused timeline without forcing every internal model mutation to become a public event.
- If the journal is auxiliary, command schemas can evolve more freely than canonical persistent schemas.

#### Costs and failure modes

- "Journal" is underspecified. A debugging log, an undo stack, an audit record, and a deterministic replay source need different content and retention guarantees.
- If both graph snapshots and a durable journal claim authority, divergence requires a recovery rule. The format needs an explicit invariant such as "snapshot at journal position N" plus integrity checks.
- Persisted commands replay requests, not accepted facts. Revalidation, defaults, selection, environment, or algorithm changes can make the same command fail or produce a different result.
- Transaction deltas are usually tied to a particular before-state. They may support undo while being unsuitable for rebuilding the document from an empty state.
- Editing old feature parameters and recomputing descendants provides CAD historical editing, but it does not by itself preserve every prior project revision.
- An auxiliary journal cannot satisfy "complete replay from origin" unless it captures all durable inputs and has a compatibility policy for every handler version.

#### Important variants

| Variant | Canonical truth | Undo | Audit | Rebuild from origin |
| --- | --- | --- | --- | --- |
| Graph plus transient deltas | Graph/project snapshot | Deltas | No durable guarantee | No |
| Graph plus durable command audit | Graph/project snapshot | Deltas or inverse commands | Human-readable intent | Not necessarily |
| Graph plus replayable commands | Graph and/or command stream | Commands/deltas | Yes | Intended, but requires deterministic/versioned handlers |
| Graph plus domain-event journal | Graph snapshot at event position | Events, compensation, or deltas | Yes | Possible if events are complete and compatible |

**Hypothesis:** The first two variants have substantially lower long-term schema and replay obligations than the last two. Calling all four "graph plus journal" would hide the main architecture decision.

### Option B: Pure event sourcing

In this family, an append-only stream of accepted domain events is canonical. The feature graph, current model, object timeline, and indexes are projections. Snapshots accelerate loading but do not replace the event stream as authority.

```text
command -> validate against projection -> domain events -> append
                    ^                         |
                    |                         v
                    +---- rebuilt feature graph/projections
```

#### Evidence

- **Fact:** Microsoft's Event Sourcing pattern defines an append-only event store as the system of record and materialized views as projections. It also identifies replay cost, eventual consistency, event evolution, ordering, idempotency, and compensating events as design concerns.[S3]
- **Fact:** The same guidance recommends snapshots as checkpoints when reconstructing state from a long event stream becomes expensive.[S3]
- **Fact:** Temporal replays recorded history against workflow code and requires workflow definitions to obey deterministic constraints. Code changes that alter command ordering require versioning or patching so old histories remain replayable.[S4]

#### Advantages

- Accepted history is preserved naturally and can drive audit, object-focused timelines, derived views, and reconstruction.
- The log position provides a natural revision identity, and projections can be rebuilt to detect corruption or projection bugs.
- Undo can be represented as a new accepted domain change rather than destructive mutation of prior records.
- Multiple projections can support current state, timeline queries, statistics, or future collaboration without changing the original events.

#### Costs and failure modes

- Event schemas become long-lived data contracts. Renaming a field is easy compared with changing the meaning or algorithm attached to an old event.
- Commands and events must remain distinct. Persisting user requests as though they were facts creates replay and validation ambiguity.
- Replaying old events through current code is unsafe unless compatibility is intentional. Keeping every historical implementation indefinitely has its own operational and security cost.
- Snapshots, projection versions, migrations, integrity checks, and replay tooling become core infrastructure before event sourcing yields reliable benefits.
- A projection bug can affect all rebuilt states, while a non-rebuildable snapshot may preserve old output but weaken the claim of deterministic replay.
- Events must have a deliberate granularity. Low-level property-change events are mechanically complete but poor user history; high-level events are understandable but couple replay to larger algorithms.
- External files, clocks, random values, locale, units, solver behavior, kernel versions, plugin versions, and concurrency decisions must be captured or constrained.

#### "Pure" event sourcing still needs CAD state structures

**Fact:** Event sourcing specifies how authoritative changes are persisted; it does not supply a parametric dependency model, topological naming, geometric algorithms, or semantic merge.[S3]

**Hypothesis:** A ThreeTerm event-sourced implementation would still maintain a feature graph as its principal projection. Therefore, the comparison is not "feature graph versus events." It is "feature graph as canonical state, with some form of journal" versus "events as canonical state, with the feature graph as a projection."

### Direct comparison

| Concern | Canonical graph plus journal | Pure event sourcing |
| --- | --- | --- |
| Current-state load | Load graph snapshot directly | Load snapshot and replay tail, or replay all events |
| Primary durable schema | Feature/object/project schema; journal schema if durable | Event schemas plus snapshot/projection schemas |
| Parametric recomputation | Native responsibility of canonical graph | Native responsibility of graph projection |
| Human-readable history | Journal must be designed for it | Event stream can provide it if event granularity is domain-level |
| Undo/redo | Deltas, inverse commands, or saved revisions | New compensating events, cursor projections, or branches |
| Historical feature edit | Mutate feature parameters in a new transaction/revision | Append an event that changes the feature in a new revision |
| Rebuild from origin | Optional and potentially unsupported | Defining property, subject to version compatibility |
| Schema longevity | Canonical data must migrate; auxiliary journal may be disposable | Every retained event meaning must remain interpretable |
| Corruption/debug recovery | Snapshots and optional journal | Event integrity plus projection rebuild and snapshots |
| MVP machinery | Can be small if journal is explicitly non-authoritative | Requires event design, replay, projection, migration, and snapshots to establish the claimed guarantees |
| Branching | Separate revision model required | Separate revision/stream ancestry model still required |
| Topological naming | Required | Required |

### Requirement-to-machinery map

The vision can be decomposed into separately testable commitments. A requirement in one row does not automatically justify the machinery in later rows.

| Claimed requirement | Minimum additional commitment to evaluate | Does not by itself imply |
| --- | --- | --- |
| Parametric recomputation | Durable feature parameters plus dependency/order semantics | Durable command history, revision branches, or runtime plugins |
| Session-local undo/redo | Reversible transactions, inverse commands, or an undo stack | Replay from project origin or preserved redo branches |
| Chronological object timeline after reopening | Durable user-level records plus affected-object indexing | Events as canonical state |
| Deterministic rebuild from origin | Complete accepted changes, resolved inputs, compatible handlers, and replay verification | Byte-identical geometry or semantic merge |
| Fast open with long history | Measured checkpoint/snapshot and cache policy | A particular database or project container |
| Named immutable versions | Stable revision identity and retained project state/log position | Divergent branches or merge |
| Continue from an old version without losing descendants | Revision parentage and more than one live reference | Automatic or semantic merge |
| Semantic CAD merge | Stable identities, schema-aware diff, conflict semantics, and naming behavior | Correct merge merely because history is a DAG |
| Internal modularity | Source boundaries and possibly static registration | Public ABI, sandbox, or independent plugin deployment |
| Lua keymaps/configuration | A bounded script API and startup recovery policy | Scripted persistent features or third-party binary plugins |
| One concrete third-party extension type | A boundary sized for that capability and trust level | One universal plugin runtime for every extension type |

**Hypothesis:** This decomposition permits evidence-driven staging without precluding later capabilities. It does not establish which row belongs in ThreeTerm's MVP; that remains a product decision.

### Deterministic replay is a separate contract

The phrase "deterministic replay" needs an equivalence target:

| Possible target | What equality means | Additional obligations |
| --- | --- | --- |
| Logical model | Same durable features, parameters, and references | Stable schemas and handler semantics |
| Dependency state | Same graph, ordering, and dirty results | Stable dependency rules and ordering |
| Geometric result | Geometrically equivalent bodies | Kernel/algorithm compatibility and numerical tolerance policy |
| Byte-identical geometry | Identical serialized B-rep/cache bytes | Serialization, platform, floating point, kernel, and ordering control |
| User-visible behavior | Same accepted/rejected actions and outputs | Stable validation, defaults, permissions, and error policy |

**Fact:** Wasmtime's deterministic execution documentation requires deterministic host imports, NaN canonicalization, deterministic relaxed SIMD, controlled memory/table growth, and deterministic fuel-based interruption instead of nondeterministic epoch interruption. This demonstrates that a deterministic instruction engine still needs deterministic host inputs and resource policy.[S18]

**Hypothesis:** Byte-identical B-rep replay across kernel upgrades or supported platforms is likely a much stronger and more expensive promise than logically or geometrically equivalent replay. This must be measured rather than assumed.

**Decision:** Define whether old projects must:

1. reproduce results using the current implementation,
2. reproduce results using the historical implementation,
3. preserve previously materialized geometry even if regeneration changes, or
4. report a controlled compatibility failure.

These policies imply different project formats and plugin lifecycles.

## Historical Editing, Undo, and Branches

"Edit a historical operation" can mean four different interactions:

1. **Edit an earlier feature in the current model.** Change its parameters now and recompute affected descendants. The old project state may or may not remain addressable.
2. **Inspect a past revision and continue from it.** Create a new line of development from an immutable past state. This is a branch even if the UI does not use that term.
3. **Correct the recorded past.** Replace or insert an old event and reinterpret all later history as though the correction had always happened. This rewrites history or creates a new derived history.
4. **Undo and redo local actions.** Move a working cursor through a linear stack, apply inverse changes, append compensations, or create revision ancestry.

**Fact:** Qt's default undo stack is linear. Executing a new command after undo deletes the commands that had been available for redo.[S5]

**Fact:** Event-sourcing guidance treats corrective changes as new events or compensating events rather than mutation of already published events.[S3]

**Fact:** Onshape preserves immutable versions separately from the current workspace. Restoring a version does not erase later history; it creates a new history entry in the workspace.[S6]

**Decision:** ThreeTerm must choose which of the four interactions the vision requires. They are not interchangeable implementations of one feature.

### Three graphs, explicitly

| Graph | Typical edge | Can be linear initially? | What a DAG adds |
| --- | --- | --- | --- |
| Feature dependency graph | Feature B consumes output of A | Only if every feature is forced into total execution order | Selective recomputation and independent dependencies |
| Operation chronology | Operation N happened after N-1 | Yes | Causality beyond total order, usually unnecessary for single-user local execution |
| Revision history | Revision B has parent A | Yes | Branches, preserved redo alternatives, merges, and ancestry |

The feature dependency graph can be a DAG while revision history remains linear. Conversely, revision history can branch while each revision contains an ordered feature list.

### Linear revision history

#### Advantages

- A single current revision and one undo/redo path have straightforward UI and storage semantics.
- No branch references, merge bases, merge commits, branch garbage collection, or multi-parent integrity rules are needed.
- Object timelines can be indexed over one total order.

#### Costs

- Starting new work after undo must either discard the redo tail, keep an unreachable archive, or silently introduce branch semantics.
- "Continue from this historical revision without losing the current work" cannot be represented as one immutable line.
- Later introduction of branches may require changing revision identity, links, synchronization assumptions, and project UX.

### Revision DAG

**Fact:** Git commit objects name a tree snapshot and one or more parent commits. Branches are movable references to commits; the commit graph carries ancestry.[S9]

**Fact:** This data structure does not itself define application-level merge. Git documents file/tree objects, while Onshape separately defines CAD comparison and merge in document/tab/feature terms.[S7][S8][S9]

**Fact:** Onshape compares versions, workspaces, and history entries at document, tab, and feature levels. Its merge behavior distinguishes tab types, inserts or updates features, can report conflicts or regeneration errors, and cannot merge every kind of incompatible change.[S7][S8]

#### Advantages

- A past revision can become a new branch without erasing descendants.
- Revision identity and common ancestry support comparison, synchronization, and merge workflows.
- Undo alternatives can be preserved rather than deleted.

#### Costs

- The project needs branch/reference lifecycle, merge-base selection, conflict representation, garbage collection or retention, and UI for detached or divergent history.
- Every persistent entity needs stable identity across revisions for useful semantic comparison.
- Multi-parent revisions require a clear result model. A merge commit cannot merely claim two parents; it must contain or derive a coherent merged feature model.
- Large derived geometry in every snapshot can make naive branch storage expensive. Structural sharing or disposable caches add their own complexity.

### Semantic diff and merge prerequisites

A revision DAG is necessary for branch ancestry but insufficient for semantic CAD merge. Useful merge also depends on:

- stable project, object, feature, parameter, and reference identities;
- schema-aware comparisons rather than serialized-byte comparisons;
- a definition of independent versus conflicting edits;
- feature-order and dependency conflict rules;
- delete/modify, rename/modify, and reorder semantics;
- plugin-defined feature and schema support;
- robust topological references after upstream changes;
- deterministic regeneration or a policy for divergent results; and
- user-facing conflict artifacts that remain valid when regeneration fails.

**Hypothesis:** Topological naming and stable feature identity are blocking dependencies for useful geometric semantic merge. Implementing a generic revision DAG first would prove ancestry and version browsing, but not the difficult CAD part of merging.

## Snapshots and Project Format

### Snapshot meanings by state model

| Snapshot role | Canonical graph model | Pure event sourcing |
| --- | --- | --- |
| Saved current model | May be the authoritative project state | Projection checkpoint, not sole authority |
| Undo checkpoint | Optional optimization around deltas | Optional projection checkpoint |
| Immutable revision | Full/structurally shared project state or journal position | Event position plus optional checkpoint |
| Derived geometry cache | Disposable if regeneration is compatible | Disposable if replay and regeneration are compatible |
| Recovery artifact | Last coherent graph plus journal tail | Verified snapshot plus event tail |

**Fact:** OCAF provides XML and binary persistence, maps application attributes through storage and retrieval drivers, and supports partial document loading. Its persistence layer must know how custom application data is converted.[S1]

**Fact:** Git commit objects refer to complete tree snapshots while the object database shares unchanged content by hash.[S9]

These are two different precedents, not candidate formats. OCAF demonstrates schema-aware document persistence and custom data drivers. Git demonstrates immutable snapshot identity and structural sharing. Neither directly supplies ThreeTerm's parametric schema or geometric compatibility rules.

### Logical project layers

Regardless of container or database, the following layers have different durability rules:

| Layer | Examples | Candidate durability |
| --- | --- | --- |
| User-authored canonical data | Features, parameters, names, expressions, constraints | Required |
| History | Commands, events, transaction records, revision parentage | Depends on selected history contract |
| Derived indexes | Object timeline index, dependency reachability, search index | Rebuildable if source data is complete |
| Derived geometry | B-reps, tessellations, thumbnails | Cache, compatibility fallback, or canonical artifact; undecided |
| Extension data | Plugin feature payloads, custom attributes, script metadata | Required if extensions may define persistent state |
| Compatibility manifest | Format, schema, kernel, algorithm, plugin, and API versions | Required to diagnose or control replay |

### Format properties to decide before technology

1. **Authority:** Which layer is the source of truth when a snapshot, journal, and cached geometry disagree?
2. **Atomicity:** What constitutes a coherent save, and how is an interrupted write detected and recovered?
3. **Integrity:** Are revisions, records, and payloads checksummed or content-addressed?
4. **Evolution:** Which schemas migrate in place, lazily, or through new events? Are old fixtures permanently supported?
5. **Unknown extension data:** Can a project preserve an unavailable plugin's payload without interpreting it, and is the project editable in that state?
6. **Capabilities:** Does opening a project require particular plugins, algorithm versions, fonts, external files, or environment inputs?
7. **Executable content:** Can a project contain scripts or extension requests, and can any of them execute automatically?
8. **Cache invalidation:** Which version fingerprint invalidates derived geometry, tessellation, indexes, and snapshots?
9. **Partial loading:** Can metadata/history be inspected without loading every body or plugin?
10. **Branch sharing:** Are unchanged objects shared between revisions, copied, or regenerated?
11. **Failure behavior:** Can the project open read-only or degraded when a plugin or historical implementation is unavailable?
12. **Portability:** What numerical, kernel, platform, endianness, and path assumptions enter persisted state?

**Hypothesis:** Choosing a ZIP layout, SQLite database, object store, or flat serialization before settling authority and compatibility semantics would create an unjustified persistence boundary. The logical contracts above can be tested independently of the final storage technology.

### Snapshot cadence

**Fact:** Event-sourcing guidance recommends snapshots based on reconstruction cost rather than treating a snapshot on every event as inherent to the pattern.[S3]

**Hypothesis:** Useful snapshot cadence will depend on representative project size, graph fan-out, geometry recomputation cost, and startup budget. Fixed event-count cadence is unlikely to track CAD workload cost well enough without measurement.

**Decision:** Determine whether snapshots are periodic replay checkpoints, explicit named versions, automatic save points, geometry caches, or several separately identified artifacts.

## Topological Naming

Topological naming is the bridge between durable parametric intent and kernel topology that may be replaced during recomputation.

### Evidence from OCCT

**Fact:** OCAF's naming service stores evolution relationships such as generated, modified, and deleted shapes. Modeling algorithms are expected to report how result topology relates to argument topology.[S1]

**Fact:** OCAF documents that selection recovery can fail or become ambiguous, especially when multiple shapes satisfy the same evolution or geometry conditions.[S1]

**Fact:** OCAF distinguishes the identity of application data labels from the identity and evolution of topological subshapes.[S1]

### Consequences for ThreeTerm

- A stable feature ID does not make a face or edge stable.
- Event sourcing can reproduce the sequence that caused topology changes, but it does not identify which new face corresponds to an old selected face.
- Exact deterministic replay may reproduce the same unstable index under identical code, but historical edits intentionally change upstream topology and can invalidate that index.
- Object-focused history needs durable object/feature IDs even before subshape naming is solved.
- Semantic branch merge that touches downstream references needs to distinguish a true conflict from a successfully remapped reference.
- Plugin-defined modeling features must participate in topology evolution reporting if downstream features can reference their output.

### Naming strategy families to investigate

| Strategy | Useful property | Known risk or open question |
| --- | --- | --- |
| Operation-provided evolution map | Uses modeling semantics: generated, modified, deleted | Every operation must report complete and correct history; ambiguity remains |
| Feature-owned semantic roles | Names outputs by intent such as "start cap" | Not every operation has stable or unique semantic roles |
| Geometric/signature matching | Can recover references without explicit operation history | Tolerance, symmetry, and coincident geometry can create unstable matches |
| Query-based references | Re-evaluates a declarative selection against new topology | A query can select zero, one, or many results after edits |
| Hybrid with explicit ambiguity | Combines evidence and refuses uncertain remaps | Requires durable diagnostics and user repair workflow |

**Decision:** Define whether ThreeTerm favors silent best-effort remapping, explicit broken references, user repair, or constrained modeling operations. This is a product behavior as much as a data structure.

## Extensibility Boundaries

### Clarify "microkernel"

The vision's "microkernel with plugins" can describe at least three independent ideas:

1. a small domain core with internal modules;
2. a runtime extension mechanism for trusted first-party components; or
3. a public third-party ecosystem with compatibility and security commitments.

**Fact:** Static registration can implement the first idea without dynamic loading. SQLite, for example, documents the same extension implementation being linked statically or loaded at run time.[S15] A runtime loader, stable ABI, sandbox, or process host is only needed for independently deployed code.

**Decision:** Identify the extension categories before selecting a runtime. File importers, modeling features, solvers, UI panels, commands, renderers, and configuration scripts do not need equal authority or call granularity.

### Option comparison

| Boundary | Loading/version contract | Isolation and trust | Call overhead and startup | Persistent-model implications |
| --- | --- | --- | --- | --- |
| Static modules | One application build; internal source/API contract | Same process and authority as core | No runtime discovery or cross-boundary marshalling; module initialization still matters | Release and migrate with core; unavailable-plugin state does not arise for shipped modules |
| Native dynamic ABI | Platform loader plus ABI/API compatibility | Same process; plugin can crash or fully compromise host | Native calls are cheap, but discovery, relocation, initialization, and dependency loading affect startup | Plugin version must remain available or data needs migration/degraded-open semantics |
| WebAssembly | Runtime plus module/component interface and capability imports | Sandboxed linear memory/control flow; host imports define actual authority | Compile/deserialize/instantiate and host-boundary costs; caching and precompilation can reduce startup | Persistent feature behavior still needs module/schema/version retention and deterministic host APIs |
| Out-of-process | Versioned IPC protocol | Process failure isolation; security requires an OS sandbox and constrained protocol, not merely a process | Process spawn, context switching, serialization, and data transfer; coarse calls amortize cost | Host must define transaction, retry, crash recovery, and unavailable-service behavior |
| Embedded Lua | Host-defined Lua API and script compatibility | Same process; authority depends on opened libraries and registered host functions | Interpreter creation, script loading, dynamic dispatch, and callbacks; workload must be measured | Configuration-only scripts can stay outside project state; persistent feature scripts create replay/migration/security obligations |

The performance statements in this table are directional **hypotheses** except where a source below documents a concrete mechanism. Representative CAD workloads must determine whether any cost is material.

### Static modules

#### Properties

- No public binary compatibility promise is required between separately shipped artifacts.
- Compiler and linker can reason across more of the program, subject to build configuration.
- First-party features can register commands, serializers, or feature kinds through internal interfaces.
- Deployment has one tested version set.

#### Limits

- Users cannot add or update compiled extensions independently.
- All shipped code shares the core's trust and failure domain.
- A poorly designed internal module interface can still create excessive abstraction even without a dynamic boundary.

**Hypothesis:** Static modules are the lowest-machinery way to investigate whether proposed extension seams are actually coherent. That does not establish whether runtime extensions are required by the product.

### Native dynamic ABI

#### Evidence

- **Fact:** Qt loads plugins dynamically and checks compatibility between the plugin and Qt build. Its deployment documentation warns that plugins built against incompatible major or higher minor versions are not loaded and that mismatched build modes can be incompatible on some platforms.[S12][S13]
- **Fact:** GCC's libstdc++ ABI documentation tracks symbol versions, compiler/runtime compatibility, and ABI changes, illustrating the ongoing compatibility surface of a C++ binary interface.[S14]
- **Fact:** SQLite supports both run-time-loaded and statically linked extensions through a C interface. Loadable extensions use defined entry points and can be disabled by default for security.[S15]

#### Advantages

- Direct access to native data and libraries can suit compute-heavy or fine-grained kernel integration.
- Existing C/C++ libraries may require less adaptation.
- Dynamic loading permits independent installation and, within compatibility rules, independent release.

#### Costs

- Exposing C++ types directly couples compiler, standard library, build flags, allocation, exceptions, RTTI, and dependency versions. A narrow C ABI with opaque handles reduces but does not eliminate API/version obligations.
- Native code has the host process's memory and system authority unless separately sandboxed.
- Plugin crashes and memory corruption affect the whole application.
- Plugin discovery and eager initialization can damage startup; lazy loading helps only when project opening or command enumeration does not require the plugin.

**Decision:** If native plugins remain an option, determine whether the stable contract is a C ABI, a same-toolchain C++ ABI, or an explicitly unstable first-party interface. The term "native plugin" does not answer this.

### WebAssembly

#### Evidence

- **Fact:** WebAssembly's security model uses a sandboxed execution environment with linear-memory bounds checks and control-flow integrity, while embedder-provided APIs determine access to host resources.[S16]
- **Fact:** Wasmtime's security documentation states that untrusted Wasm can be sandboxed but also requires careful configuration of host functionality, limits, and runtime security.[S17]
- **Fact:** Wasmtime documents ahead-of-time compilation, serialized precompiled modules, faster compilation configurations, and copy-on-write memory initialization as ways to change compile and instantiation costs.[S18]
- **Fact:** The WebAssembly Component Model's WIT defines language-independent interfaces in terms of typed functions and resources.[S19]

#### Advantages

- The host can expose explicit capabilities rather than ambient process authority.
- A typed language-neutral interface can decouple guest implementation language from host internals.
- Runtime validation and isolation provide a stronger untrusted-code boundary than an in-process native ABI.
- Fuel, memory limits, and host-call policy can constrain some denial-of-service behavior.

#### Costs

- Compilation or deserialization and instantiation affect cold start; precompilation and caches then become deployment/version artifacts.
- Fine-grained traversal of a native CAD object graph through host calls is likely expensive and awkward. Coarse, typed operations or transferred representations need benchmarking.
- A sandbox does not guarantee deterministic behavior if imports expose clocks, randomness, files, networks, nondeterministic ordering, or version-varying host algorithms.
- Persistent plugin-defined features still need module identity, schema migration, version retention, naming behavior, and headless availability.
- Debugging, profiling, large geometry exchange, and native dependency integration add toolchain complexity.

### Out-of-process extensions

#### Evidence

- **Fact:** The Language Server Protocol uses JSON-RPC between a development tool and a language server, allowing one server to serve multiple clients across a process boundary.[S20]
- **Fact:** VS Code runs extensions in one or more extension hosts selected by extension kind and environment, including local, web, and remote hosts.[S21]

These systems show that a process/protocol boundary can support independently deployed capabilities. They do not establish that JSON-RPC or VS Code's topology fits high-volume CAD geometry.

#### Advantages

- A crashed extension process can be detected and restarted without necessarily corrupting the UI process.
- The protocol can be language- and compiler-neutral.
- OS-level sandboxing, resource limits, and separate credentials can be applied when the deployment supports them.
- Coarse tasks such as import/export, batch solving, or rendering may fit request/response or streaming protocols.

#### Costs

- A separate process without OS sandboxing may still have the user's filesystem and network authority.
- Calls require serialization, copying or shared-memory coordination, cancellation, deadlines, error mapping, and protocol versioning.
- Transferring large B-reps or making per-face calls can dominate useful work.
- Model mutation requires a transaction protocol: the host must remain authoritative if the extension times out or crashes midway.
- Startup may involve process spawn and runtime initialization; keeping hosts warm trades latency for memory.

### Embedded Lua

#### Evidence

- **Fact:** Lua is designed to be embedded, and its C API lets the host create states, register functions, load chunks, and decide which standard libraries to open.[S22]
- **Fact:** Lua's standard libraries include `io`, `os`, `package`, and `debug` facilities. The debug API also provides hooks that a host can use for execution control or instrumentation.[S22]

#### Suitable scopes to investigate

- user-owned configuration and keymaps;
- composition of already-authorized commands;
- startup customization with a clear failure fallback;
- project automation invoked explicitly by the user; and
- full scripted parametric features, which have much larger persistence obligations.

#### Costs and risks

- Opening powerful standard libraries or host functions grants filesystem, process, dynamic-loading, or model authority. "It is Lua" is not a security policy.
- An infinite or expensive script can block an in-process host unless execution is budgeted, interruptible, or isolated.
- Dynamic argument and return contracts move compatibility failures to runtime unless the command schema performs validation.
- Auto-running project-provided scripts would make opening a data file an execution boundary.
- If Lua scripts define persistent geometry, script source/version, dependencies, determinism, errors, and missing-script behavior become part of the project format.

**Hypothesis:** Local configuration and keymaps are a narrower trust and compatibility problem than general plugins. Treating Lua configuration and persistent feature scripting as one capability would unnecessarily couple them.

## Versioning Is a Stack, Not One Number

| Version layer | Protects against | Example failure if omitted |
| --- | --- | --- |
| Application/build version | Incompatible first-party components | Static module and core disagree on internal assumptions |
| Native ABI version | Binary layout/calling convention changes | Loader succeeds but calls corrupt memory |
| Host API or IPC protocol version | Semantic interface changes | Extension sends valid syntax with obsolete meaning |
| Command schema/version | Changed arguments, defaults, outputs, or permissions | Headless scripts or journals replay differently |
| Domain-event schema/version | Changed durable fact representation or meaning | Old project cannot rebuild projections |
| Project-format version | Container and canonical-data evolution | Reader misinterprets saved state |
| Algorithm/kernel fingerprint | Changed modeling result for same logical input | Replay produces different topology or geometry |
| Lua API version | Renamed commands or changed values | User configuration fails at startup |
| Plugin feature schema | Plugin-owned persistent payload evolution | Project cannot load or migrate feature data |

**Fact:** Protocol Buffers' evolution rules are an example of a schema technology with explicit field-number and compatibility constraints. Such a technology can help preserve unknown data, but it cannot preserve changed domain meaning automatically.[S23]

**Hypothesis:** A single semantic version for ThreeTerm cannot express all of these compatibility dimensions. The project manifest and diagnostics may need to distinguish them even if the implementation initially ships them in lockstep.

## Security and Trust Boundaries

The extension choice depends on who supplies code and when it executes.

| Source | Default trust question | Principal risk |
| --- | --- | --- |
| Built-in first-party module | Is the release itself trusted? | Ordinary application defects |
| User-installed native plugin | Does the user accept full process authority? | Memory corruption and ambient system access |
| Downloaded Wasm module | Which host capabilities are granted? | Host API misuse, resource exhaustion, sandbox/runtime defects |
| Out-of-process extension | Is it OS-sandboxed and what credentials does it inherit? | Filesystem/network access and protocol abuse |
| User-owned Lua config | Is startup failure recoverable and can config access the OS? | Lockout, data loss, arbitrary local actions |
| Project-embedded script/plugin reference | Is a document allowed to cause code execution? | Untrusted-file-to-code-execution path |

**Decision:** Establish at least three policies independently:

1. installation trust: what a user must approve;
2. capability trust: what the extension can access; and
3. activation trust: whether loading or previewing a project can execute extension code.

No runtime choice removes the need for those policies.

## Plugin Capability Determines Architecture Cost

"Plugins" should be decomposed by what they may own:

| Capability | Persistence/replay coupling | Interface pressure |
| --- | --- | --- |
| Add a presentation or keybinding | Low if it does not mutate project state | UI/TUI and command metadata |
| Add a command composed from core operations | Medium if command is journaled | Typed command API and permissions |
| Import/export files | Medium; external format and deterministic import inputs matter | Coarse document/geometry transfer |
| Add a solver or analysis | Medium to high depending on persisted results | Large data, cancellation, reproducibility |
| Add a parametric feature | Very high | Schema, migration, naming, regeneration, undo, headless, replay, diff/merge |
| Add a storage codec | Very high | Project availability, integrity, long-term compatibility |

**Hypothesis:** A plugin API that only registers commands over durable core operations can be much smaller and more stable than an API that exposes mutable kernel internals. However, it may be insufficient for novel modeling features and could force inefficient command granularity.

**Decision:** Decide the first required third-party capability before stabilizing any plugin boundary. Designing for the union of all rows would conflict directly with the smallest coherent MVP constraint.

## Shared TUI and Headless Command Registry

A command registry can provide one application action surface without coupling presentation code to mutation logic.

```text
TUI key/menu --------+
headless request ----+--> parse/validate explicit invocation
Lua call ------------+                 |
plugin contribution -+                 v
                                command handler
                                  /        \
                         graph transaction  emitted events
                                  \        /
                            structured result
```

The lower half can support either state option:

- With a canonical graph, the handler validates and commits a graph transaction, optionally recording a command, event, or delta.
- With event sourcing, the handler validates against the current projection and emits accepted domain events.

### Evidence

- **Fact:** Blender operators are actions that can be invoked from UI elements, keymaps, operator search, and Python.[S10]
- **Fact:** Blender also documents that operators depend on execution context and can fail their poll when invoked from an unsuitable context. This is a warning against making focus, active panels, or implicit selection part of a headless contract.[S10]
- **Fact:** VS Code commands use string identifiers and can be invoked programmatically. Extensions can register and execute commands through the same command surface.[S11]

### Candidate command metadata

The following are comparison dimensions, not a selected schema:

| Field/behavior | Why it matters |
| --- | --- |
| Stable command ID | Keymaps, scripts, help, and plugin ownership need an identity |
| Argument and result schema | TUI prompts, headless validation, Lua errors, and documentation can share types |
| Schema/semantic version | Persisted journals or scripts may outlive handler changes |
| Explicit model targets | Headless execution cannot depend on focused panels or hidden selection |
| Availability predicate | UI can explain disabled actions before invocation |
| Declared permissions/capabilities | Hosts can mediate filesystem, network, process, or model access |
| Mutation/read scope | Transactions, object timelines, and conflict detection can identify affected objects |
| Determinism classification | Replay can reject or capture nondeterministic inputs |
| Undo/history policy | UI gestures can be grouped into one meaningful operation |
| Structured diagnostics | TUI and headless callers can render the same failure differently |
| Owner and lifecycle | Missing plugins and command ID conflicts can be diagnosed |

### Boundary rules to investigate

1. Commands should receive an explicit domain context, not UI widget state.
2. Interactive prompting belongs in a caller/adaptor. A replayable invocation must already contain resolved arguments.
3. Queries should not be forced into mutation commands merely to reuse dispatch.
4. High-frequency preview changes may need coalescing so a drag does not create hundreds of user-visible history entries.
5. A command can affect multiple objects; an object-focused timeline therefore needs affected-object metadata or an index, not a one-command/one-object assumption.
6. Command acceptance and domain events should remain distinguishable so failed intent is not persisted as changed state.
7. If commands are persisted, defaults must be resolved before recording or versioned explicitly. Replaying an omitted argument against a new default changes meaning.
8. Headless output should be structured data plus diagnostics, not captured terminal formatting.
9. Availability checks must be cheap enough for TUI refresh or evaluated on demand; they should not mutate state.

**Hypothesis:** A presentation-neutral command surface is a cross-option invariant worth testing because TUI, headless, Lua, and plugin requirements all depend on it. The public stability and persistence of that surface remain separate decisions.

## Performance and Startup

The vision requires live modeling to remain responsive as history grows and warns against sacrificing startup/performance for extensibility. Those are workload questions, not properties that can be inferred from architecture names.

### Potential critical paths

| Path | Likely variables | Measurements needed |
| --- | --- | --- |
| Cold project open | Format parsing, event tail, snapshot age, plugin activation, geometry cache | Time to metadata, first inspectable model, first rendered model, peak memory |
| Warm project open | OS cache, compiled extension cache, geometry cache | Same metrics, clearly separated from cold run |
| Historical parameter edit | Dependency fan-out, topology changes, solver/kernel cost | Dirty features, recomputed features, latency percentiles, cancellation behavior |
| Undo/redo | Delta size, event compensation, cache reuse | Latency and memory over long sessions |
| Timeline query | Event count, affected-object index, revision count | Query latency by object and date range |
| Branch switch/compare | Snapshot sharing, projection rebuild, geometry cache | Switch latency, storage amplification, comparison latency |
| Extension discovery | Filesystem scan, signature/manifest parse, dynamic loader | Startup cost with 0, 10, 100 extensions |
| First extension call | Native relocation, Wasm compile/instantiate, process spawn, Lua load | Cold and warm first-call latency |
| Geometry boundary calls | Call count, payload size, copy/shared-memory strategy | Throughput and latency for coarse and fine operations |

### Evidence-backed mechanisms

- **Fact:** Event replay cost grows with retained history unless snapshots or other checkpoints limit reconstruction work.[S3]
- **Fact:** OCAF's dependency mechanism identifies affected functions for regeneration rather than requiring every function to execute after every edit.[S1]
- **Fact:** Qt can inspect plugin metadata without instantiating a plugin, providing one example of separating discovery from activation.[S12]
- **Fact:** Wasmtime compilation, precompilation, deserialization, and copy-on-write initialization make different startup tradeoffs.[S18]

### Hypotheses to avoid treating as conclusions

- Static linkage will probably minimize boundary overhead, but application initialization and binary size may still dominate cold start.
- Native calls will probably outperform fine-grained Wasm or IPC calls, but coarse CAD operations may make the difference irrelevant.
- Event replay may be cheap compared with geometry regeneration for realistic projects, or it may dominate metadata-only startup. Both are plausible without fixtures.
- Saving derived geometry may improve first render but increase project size and compatibility burden.
- Lazy plugin activation may improve empty startup but cannot help when the opened project contains plugin-defined canonical features.

No extension runtime or state model should receive a performance verdict without the representative benchmarks below.

## Contradictions and Unresolved Semantics

These are tensions in the requirements, not proof that either side must be removed.

| Tension | Why both cannot be assumed simultaneously | Clarification or evidence needed |
| --- | --- | --- |
| Immutable complete history vs editing the past | Append-only records cannot be silently rewritten while retaining the same identity | Choose parameter edit, branch-from-past, compensation, or true history rewrite semantics |
| Linear undo vs preserved alternatives | A new action after undo either deletes redo or creates divergence | Decide whether abandoned redo must remain addressable |
| Deterministic replay vs evolving kernels/plugins | Same logical input can run different implementation code | Define equivalence target and version-retention policy |
| Fast snapshot load vs replay as authority | Trusting a snapshot skips proof by replay; verifying it costs time | Define integrity and background/foreground verification behavior |
| Semantic merge vs unstable topology | Stable feature ancestry does not stabilize face/edge references | Establish naming quality and conflict behavior first |
| Microkernel/plugins vs smallest coherent MVP | A public runtime boundary adds APIs, lifecycle, packaging, security, and compatibility before a consumer exists | Identify the first concrete external extension category |
| Plugin freedom vs durable project availability | Plugin-defined features make opening and replay depend on third-party code | Define missing-plugin and long-term retention policy |
| Lua simplicity vs document security | Configuration is local code; project-embedded scripts turn file open into code activation | Separate trust domains and activation rules |
| Fine-grained native model access vs isolation | Rich pointer-level access is fast but cannot cross a sandbox/process boundary cleanly | Benchmark representative coarse and fine plugin operations |
| Object-focused timeline vs operation atomicity | One operation can affect many objects and one object can be affected indirectly | Define direct/indirect provenance and timeline indexing |
| Lazy extension startup vs plugin-owned features | Project decoding/regeneration may require activation immediately | Decide whether unknown plugin data can be inspected or preserved without execution |
| Disposable geometry cache vs historical fidelity | Regeneration may change under new algorithms | Define whether old materialized geometry is evidence, fallback, or irrelevant cache |
| Human-readable events vs implementation-complete replay | Domain-level events omit low-level detail; low-level events obscure intent | Select event granularity and capture deterministic inputs explicitly |

## Decision Dependencies

The order of decisions matters. The following dependencies reduce the risk of designing machinery around an undefined promise.

```text
meaning of "historical edit"
        -> revision semantics
        -> linear history or DAG
        -> snapshot identity and branch UX

replay equivalence target
        -> canonical source of truth
        -> event/command schema obligations
        -> algorithm/plugin retention
        -> cache and project-format policy

stable object and topological identity
        -> object timeline quality
        -> semantic diff
        -> semantic merge feasibility

first extension capability + trust model
        -> interface granularity
        -> static/native/Wasm/process/Lua boundary
        -> startup and security model
        -> plugin data in project format

command persistence policy
        -> command schema stability
        -> TUI/headless/Lua compatibility
        -> journal/replay semantics
```

### Decisions that block others

| Decide first | Decisions it unlocks |
| --- | --- |
| What must be replay-equivalent | Event sourcing viability, snapshot authority, cache retention, version manifests |
| What historical editing means | Linear versus DAG revisions, undo semantics, branch UX |
| What constitutes a user-visible operation | Command/event granularity, object timeline, undo grouping |
| Stable ID and topological-reference policy | Semantic diff/merge and plugin feature contract |
| First third-party extension use case | Runtime boundary, API surface, packaging, threat model |
| Whether projects can execute code | Lua/plugin activation, project trust prompts, headless safety |
| Startup and edit latency budgets with fixtures | Snapshot cadence, cache policy, extension activation strategy |

## Concrete Evidence Spikes

These spikes are intended to answer decisions, not become production architecture. Results should be retained as fixtures, measurements, and short conclusions.

### E1: History semantics scenarios

**Question:** What do users expect "edit a historical operation," undo after new work, and restore to mean?

**Method:** Use a paper or throwaway interaction prototype with one model containing five named features. Walk through:

1. edit feature 2 while viewing the current model;
2. undo features 5 and 4, then create a different feature;
3. inspect revision R2 and continue working;
4. restore an old version while preserving current work; and
5. compare two divergent outcomes.

**Record:** Which prior states remain addressable, what the timeline displays, whether the user expects a branch, and whether later operations are recomputed, copied, or discarded.

**Exit criterion:** One explicit semantic definition for each scenario. This unlocks linear versus DAG revision decisions.

### E2: Deterministic replay envelope

**Question:** Which replay equivalence can the geometry stack actually support?

**Fixture:** A small corpus covering sketches/constraints, extrusion, boolean operations, fillets/chamfers, patterns, topology-changing parameter edits, imported geometry, and failed regeneration.

**Method:** Record resolved logical inputs, operation order, dependency versions, and outputs. Replay across repeated runs, supported machines, thread configurations, and at least two kernel/algorithm builds.

**Measure:** Logical graph equality, stable IDs, topology counts, geometric tolerance equivalence, serialized-byte equality, error equality, and elapsed time.

**Exit criterion:** A documented replay target and list of required captured inputs/version pins. This unlocks canonical-state and snapshot policy decisions.

### E3: Topological naming corpus

**Question:** How often can durable references survive representative historical edits?

**Fixture:** References to faces, edges, and vertices downstream of booleans, fillets, patterns, mirrored features, split/merged faces, symmetric geometry, and operation reorderings.

**Compare:** Operation history maps, semantic roles, geometric signatures, query references, and a hybrid that reports ambiguity.

**Measure:** Correct remap, explicit break, false remap, ambiguity, stability across replay, and diagnostic quality. False remaps should be scored separately from visible failures because silent wrong geometry is more dangerous.

**Exit criterion:** A naming strategy envelope and failure policy. This unlocks semantic diff/merge claims and the persistent feature-plugin contract.

### E4: Recompute and history-growth benchmark

**Question:** What actually limits interactive edits as history grows?

**Fixture:** Models with deep chains, wide independent branches, high fan-out, repeated patterns, expensive booleans, and intentionally failed downstream features at increasing sizes.

**Compare:** Ordered full-tail regeneration, dependency-directed regeneration, and any proposed cache/checkpoint strategy.

**Measure:** Dirty-node discovery, nodes recomputed, kernel time, scheduler overhead, memory, cancellation latency, and p50/p95 edit latency.

**Exit criterion:** A measured latency budget and evidence for whether a dependency DAG, caching, or snapshots are necessary at MVP scale.

### E5: Project snapshot and recovery fixtures

**Question:** Which logical format layers are necessary for startup, migration, and recovery?

**Method:** Encode the same small projects under the state alternatives using disposable prototype encodings. Include an interrupted write, corrupt record, old schema, unknown plugin payload, missing plugin, stale geometry cache, and long history with a recent snapshot.

**Measure:** Time to metadata, time to editable state, time to first geometry, project size, recovery result, unknown-data preservation, migration diagnostics, and replay verification cost.

**Exit criterion:** An authority/integrity contract and required manifest fields. Do not select the production container solely from this spike.

### E6: Extension boundary benchmark

**Question:** Which boundaries are viable for the first concrete extension workload?

**Implement the same operations:** one metadata-only command, one coarse geometry operation, one high-frequency property query, one large geometry transfer, and one failing or nonterminating extension.

**Compare where feasible:** static call, narrow native C ABI, WebAssembly host API, out-of-process protocol, and Lua for workloads it can express.

**Measure:** Application cold start, discovery, cold/warm first invocation, calls per second, transfer throughput, memory, cancellation, crash containment, diagnostics, and packaging size.

**Exit criterion:** A measured boundary recommendation for a named capability and trust level, not for an abstract universal "plugin."

### E7: Command-surface parity

**Question:** Can TUI, headless, and Lua callers invoke identical domain behavior without hidden UI context?

**Fixture:** Create/edit/delete a feature, inspect an object, trigger a failed validation, perform a multi-object command, invoke undo, and run a preview/coalesced edit.

**Method:** Run equivalent invocations through each caller and compare accepted command arguments, emitted events or graph transactions, structured results, affected-object provenance, and diagnostics.

**Exit criterion:** A minimal presentation-neutral command contract and a list of interactions that should remain caller-specific.

### E8: Extension and project threat model

**Question:** Which code and data origins are trusted, and what can each activate?

**Method:** Threat-model built-ins, installed native plugins, downloaded Wasm, extension processes, local Lua config, and project-embedded script/plugin references. Include file-open, preview, headless CI, and project-sharing paths.

**Record:** Assets, authorities, activation events, persistence, denial-of-service limits, recovery mode, signing/approval assumptions, and audit requirements.

**Exit criterion:** A capability and activation policy that can reject runtime options whose isolation is insufficient.

## Option-Neutral Invariants Worth Testing

These observations narrow risk without selecting a final architecture:

1. Stable IDs for project objects and features are useful under every state and history option.
2. Mutation logic that does not depend on TUI widget/focus state is useful for TUI, headless, Lua, replay, and tests.
3. Explicit dependency metadata is needed for selective recomputation whether the graph is canonical or projected.
4. Commands, accepted domain events, transaction deltas, and revision identities should use distinct terminology even if an MVP stores only some of them.
5. Derived caches need an explicit invalidation/version policy and must not silently compete with canonical state.
6. Extension-owned persistent features require more compatibility machinery than presentation or import/export extensions.
7. A sandbox only controls capabilities that do not leak through host APIs.
8. A revision DAG does not make merge semantic, and an event log does not make replay deterministic.

## What Remains Undecided

This research intentionally leaves the following open:

- whether the canonical source is a feature graph/project snapshot or an event stream;
- whether any command journal is transient, auditable, or authoritative for replay;
- whether persisted records are commands, domain events, transaction deltas, revisions, or a deliberate combination;
- the required deterministic replay equivalence and historical-version policy;
- what "edit a historical operation" and "restore" mean to users;
- whether MVP revision history is destructive linear history, immutable linear history, or a DAG;
- whether and when semantic branching and merge are in scope;
- the stable identity and topological naming strategy;
- snapshot roles, cadence, and authority;
- whether derived geometry is disposable cache, compatibility fallback, or canonical artifact;
- the project container, database, encoding, and migration mechanism;
- the first third-party extension capability and its trust model;
- whether runtime extensions use native ABI, WebAssembly, processes, Lua, multiple boundaries, or none initially;
- whether Lua is configuration-only, command automation, or a persistent feature language;
- whether projects can contain or automatically activate executable content;
- command registry schema, public stability, persistence, and versioning;
- concrete startup, edit-latency, project-size, and extension-overhead budgets.

## Sources

Sources were accessed 2026-07-30. Project documentation and specifications are preferred; system examples are precedents, not endorsements.

- **[S1]** Open CASCADE Technology, *OCAF User's Guide*: document model, transactions/deltas, function dependency/regeneration, naming, and persistence. <https://dev.opencascade.org/doc/overview/html/occt_user_guides__ocaf.html>
- **[S2]** Onshape, *Feature List*: ordered feature history, rollback, and regeneration behavior. <https://cad.onshape.com/help/Content/feature_list.htm>
- **[S3]** Microsoft Azure Architecture Center, *Event Sourcing pattern*: append-only event authority, projections, snapshots, replay, compensation, and pattern tradeoffs. <https://learn.microsoft.com/en-us/azure/architecture/patterns/event-sourcing>
- **[S4]** Temporal, *Workflow definition and deterministic constraints*: replay compatibility and deterministic workflow code. <https://docs.temporal.io/workflow-definition#deterministic-constraints>
- **[S5]** Qt 6, *QUndoStack*: linear undo/redo stack, command deletion after divergent edits, clean state, and command grouping. <https://doc.qt.io/qt-6/qundostack.html>
- **[S6]** Onshape, *Versions and history*: immutable versions, workspaces, history entries, and restore behavior. <https://cad.onshape.com/help/Content/Document/versions_and_history.htm>
- **[S7]** Onshape, *Compare*: document, tab, and feature comparisons between versions, workspaces, and history entries. <https://cad.onshape.com/help/Content/Document/compare.htm>
- **[S8]** Onshape, *Merging*: branch/workspace merge semantics, feature changes, conflicts, and unsupported/incompatible cases. <https://cad.onshape.com/help/Content/Document/merging.htm>
- **[S9]** Git, *Git Internals - Git Objects*: tree snapshots, commit parents, object identity, and branch references. <https://git-scm.com/book/en/v2/Git-Internals-Git-Objects>
- **[S10]** Blender, *Operators*: reusable operator invocation, context, poll, undo, UI, keymaps, and Python. <https://developer.blender.org/docs/features/interface/operators/>
- **[S11]** Visual Studio Code, *Commands*: command identifiers, registration, execution, and extension command use. <https://code.visualstudio.com/api/extension-guides/command>
- **[S12]** Qt 6, *How to Create Qt Plugins* and *QPluginLoader*: plugin metadata, discovery/loading, and compatibility. <https://doc.qt.io/qt-6/plugins-howto.html> and <https://doc.qt.io/qt-6/qpluginloader.html>
- **[S13]** Qt 6, *Deploying Plugins*: plugin build/version compatibility and deployment rules. <https://doc.qt.io/qt-6/deployment-plugins.html>
- **[S14]** GCC, *libstdc++ ABI Policy and Guidelines*: symbol versioning and C++ runtime ABI compatibility. <https://gcc.gnu.org/onlinedocs/libstdc++/manual/abi.html>
- **[S15]** SQLite, *Run-Time Loadable Extensions*: C extension entry points, dynamic/static linkage, and load-extension security control. <https://www.sqlite.org/loadext.html>
- **[S16]** WebAssembly, *Security*: sandbox, linear-memory bounds, control-flow integrity, and embedder responsibilities. <https://webassembly.org/docs/security/>
- **[S17]** Wasmtime, *Security*: untrusted Wasm threat model, sandbox scope, host configuration, and limits. <https://docs.wasmtime.dev/security.html>
- **[S18]** Wasmtime, examples for *Pre-Compiling WebAssembly*, *Fast Compilation*, *Fast Instantiation*, and *Deterministic Wasm Execution*. <https://docs.wasmtime.dev/examples-pre-compiling-wasm.html>, <https://docs.wasmtime.dev/examples-fast-compilation.html>, <https://docs.wasmtime.dev/examples-fast-instantiation.html>, and <https://docs.wasmtime.dev/examples-deterministic-wasm-execution.html>
- **[S19]** Bytecode Alliance, *WebAssembly Component Model - WIT*: typed language-neutral component interfaces and resources. <https://component-model.bytecodealliance.org/design/wit.html>
- **[S20]** Microsoft, *Language Server Protocol overview*: client/server JSON-RPC boundary and protocol reuse. <https://microsoft.github.io/language-server-protocol/overviews/lsp/overview/>
- **[S21]** Visual Studio Code, *Extension Host*: local, web, and remote extension-host placement. <https://code.visualstudio.com/api/advanced-topics/extension-host>
- **[S22]** Lua 5.4 Reference Manual: embedding/C API, loading libraries, `io`, `os`, `package`, `debug`, and hooks. <https://www.lua.org/manual/5.4/manual.html>
- **[S23]** Protocol Buffers, *Proto3 Language Guide - Updating A Message Type*: schema evolution and field compatibility rules. <https://protobuf.dev/programming-guides/proto3/#updating>

[S1]: https://dev.opencascade.org/doc/overview/html/occt_user_guides__ocaf.html
[S2]: https://cad.onshape.com/help/Content/feature_list.htm
[S3]: https://learn.microsoft.com/en-us/azure/architecture/patterns/event-sourcing
[S4]: https://docs.temporal.io/workflow-definition#deterministic-constraints
[S5]: https://doc.qt.io/qt-6/qundostack.html
[S6]: https://cad.onshape.com/help/Content/Document/versions_and_history.htm
[S7]: https://cad.onshape.com/help/Content/Document/compare.htm
[S8]: https://cad.onshape.com/help/Content/Document/merging.htm
[S9]: https://git-scm.com/book/en/v2/Git-Internals-Git-Objects
[S10]: https://developer.blender.org/docs/features/interface/operators/
[S11]: https://code.visualstudio.com/api/extension-guides/command
[S12]: https://doc.qt.io/qt-6/plugins-howto.html
[S13]: https://doc.qt.io/qt-6/deployment-plugins.html
[S14]: https://gcc.gnu.org/onlinedocs/libstdc++/manual/abi.html
[S15]: https://www.sqlite.org/loadext.html
[S16]: https://webassembly.org/docs/security/
[S17]: https://docs.wasmtime.dev/security.html
[S18]: https://docs.wasmtime.dev/examples-deterministic-wasm-execution.html
[S19]: https://component-model.bytecodealliance.org/design/wit.html
[S20]: https://microsoft.github.io/language-server-protocol/overviews/lsp/overview/
[S21]: https://code.visualstudio.com/api/advanced-topics/extension-host
[S22]: https://www.lua.org/manual/5.4/manual.html
[S23]: https://protobuf.dev/programming-guides/proto3/#updating
