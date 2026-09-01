# Six-Gate Performance-Claims Gate

Status: required pre-release runbook. The product owner signs this runbook
before any ThreeTerm performance claim is published in release notes, marketing,
or the project README.

## Decision Rule

This runbook is the release-process hard stop for performance claims:

- A claim has one named evidence artifact, one declared fixture and project
  scale, and one signed run record.
- Every gate below must be `PASS` against that artifact. Missing, stale, or
  inferred evidence is `FAIL`; it is never silently treated as a pass.
- The product owner must sign all six gates. An unsigned gate refuses the claim.
- A measured band is not an MVP target. A per-class MVP target is admitted only
  when all six gates pass in the same run.
- If a gate fails, do not publish the claim or target. Record the band as
  refused, state the missing evidence, and rerun the gate against a new current
  artifact.

This rule covers latency, throughput, memory, startup, input, viewport,
selection, help, tree, timeline, load, save, replay, recompute, export, Lua,
plugin, and any other per-class or product-wide performance claim. A claim that
is outside the measured fixture is refused rather than generalized.

For this runbook, a **measured band** is an observed result tied to one
evidence artifact, fixture, profile, scale, and method; it is not a product
promise. An **admitted target** is a measured band that passed all six gates and
was signed by the product owner. A **fixture** is the named workflow and
project data, together with the renderer and terminal path, that produced the
measurement. A band or fixture may change only through a new evidence artifact
and a new signed run.

## Source Decisions

The six-gate structure is the executable form of the closed Wayfinder decision
[Set evidence-based performance budgets (#48)](https://github.com/rafaelromao/threeterm/issues/48).
The first reference workflow and its bands come from [Spike the complete
first-part workflow (#50)](https://github.com/rafaelromao/threeterm/issues/50).
The latest two-release-candidate evidence is produced by [22v2 rehearsal two
release candidates (#280)](https://github.com/rafaelromao/threeterm/issues/280)
and packaged by [22v3 rehearsal evidence artifact
(#281)](https://github.com/rafaelromao/threeterm/issues/281).

## How To Run

1. Identify the release candidates, host, container, Ghostty baseline, fixture,
   and project scale before measuring.
2. Generate or obtain the evidence artifact in
   `docs/research/rehearsal-evidence/`. For the L-bracket reference run, the
   production command is:

   ```sh
   cargo run -p threeterm-cli --bin threeterm -- \
     --machine rehearse \
     --output-dir docs/research/rehearsal-evidence/l-bracket \
     --release-candidate rc-1
   ```

   The command produces `rc-1` and `rc-2` runs and an aggregate
   `sha256-manifest.json`.
3. Confirm the checked-in catalog and artifact hashes with the existing
   integration test:

   ```sh
   cargo test -p threeterm-cli --test rehearsal_e2e \
     committed_rehearsal_evidence_has_a_reproducible_sha256_catalog
   ```

   This verifies catalog structure, file identity, and reproducible hashes. It
   does not prove the hardware, scale, statistical, Ghostty, limitations, or
   export-divergence gates below; those fields require explicit inspection and
   owner sign-off.
4. Complete all six gate sections and the per-class decision table. Do not
   pool samples from the two release candidates to satisfy a sample threshold.
5. Publish only the admitted rows. Keep refused rows and their reasoning with
   the release record.

## Evidence Identity

Complete this block before signing any gate. The values identify the exact
artifact, not merely the command that produced it.

| Field | Value |
| --- | --- |
| Evidence root | ______________________________________________ |
| Aggregate manifest path | ______________________________________________ |
| Aggregate manifest SHA-256 | ______________________________________________ |
| Evidence-producing repository commit | ______________________________________________ |
| Release candidates | ______________________________________________ |
| Rehearsal date and time (UTC) | ______________________________________________ |
| Generation command or artifact source | ______________________________________________ |
| Catalog verification command and result | ______________________________________________ |
| Product owner | ______________________________________________ |

The evidence root must be checked into the rehearsal spike evidence directory
or be attached to the release record with an immutable digest. A path in `/tmp`,
an uncommitted local file, or an artifact without a digest is not current
evidence.

## Six Gates

### Gate 1: Rehearsal Spike

**Pass only when all of these are true:**

- The current rehearsal spike ran on the L-bracket reference part through the
  workflow represented by the claim.
- The aggregate artifact and both release-candidate run directories are present
  under the rehearsal spike evidence directory.
- The catalog verification command passes, and every artifact digest resolves to
  the checked-in files.
- The evidence names the product/repository commit and candidate identifiers,
  so currentness can be checked against the release being claimed.

Evidence pointer: ______________________________________________

Status: `[ ] PASS`  `[ ] FAIL`

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Sign-off date (UTC): ______________________________________________

Reasoning or required follow-up: ______________________________________________

### Gate 2: Hardware Profile

**Pass only when all of these are true:**

- The artifact declares the CPU model, core/thread count, memory, kernel and
  microcode where relevant, container runtime and image digest, and pinned
  package/toolchain versions.
- It declares the Ghostty version, `TERM`/`TERM_PROGRAM` values, and direct
  local attachment topology. tmux, SSH, and another terminal are not silently
  substituted for the declared Official Interactive Environment.
- Both release candidates and the claim run use the same declared profile.
  A different profile creates a new band and requires a new gate run.

Record the exact profile here or point to its artifact field:

```text
CPU and cores/threads: ______________________________________________
Memory: _____________________________________________________________
Kernel/microcode: ___________________________________________________
Container runtime/image/digest: ______________________________________
Rust, OCCT, libslvs, lib3mf, and other pinned versions: ______________
Ghostty version and topology: ________________________________________
```

Evidence pointer: ______________________________________________

Status: `[ ] PASS`  `[ ] FAIL`

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Sign-off date (UTC): ______________________________________________

Reasoning or required follow-up: ______________________________________________

### Gate 3: Project Scale

**Pass only when all of these are true:**

- The artifact declares the feature count, transaction count, and `Derived
  Result` count for each fixture and claim scope.
- The two release candidates use the same scale, and the claim does not exceed
  that envelope. Growth of any count requires another gate run.
- Counts are evidence fields, not values inferred from a source file, a timing
  name, or an assumed representative project.

```text
Fixture: __________________  Claim scope: ____________________________
Feature count: _____________  Transaction count: ______________________
Derived Result count: ______  Scale notes: ____________________________
```

Evidence pointer: ______________________________________________

Status: `[ ] PASS`  `[ ] FAIL`

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Sign-off date (UTC): ______________________________________________

Reasoning or required follow-up: ______________________________________________

### Gate 4: Statistical Method

**Pass only when all of these are true:**

- The artifact names the statistical method and units.
- Sample count `n` is recorded separately for every class and every release
  candidate. Samples from `rc-1` and `rc-2` are never pooled.
- Per-class p50, p95, and p99 are reported for publication only when that class
  has `n >= 30` independent samples under the declared method. A smaller `n`
  is a measured band only and cannot become an MVP target.
- One-run timings, symbolic values, and a p50/p95/p99 triple copied from a
  single sample are not statistical evidence for a target.

```text
Method: ____________________________  Units: _________________________
Independent sample definition: _______________________________________
Minimum n met for every claimed class:  [ ] YES  [ ] NO
```

Evidence pointer: ______________________________________________

Status: `[ ] PASS`  `[ ] FAIL`

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Sign-off date (UTC): ______________________________________________

Reasoning or required follow-up: ______________________________________________

### Gate 5: Fixture-vs-Product Limits

**Pass only when all of these are true:**

- The evidence publishes a named limitations document alongside the artifact.
- The limitations document states the fixture, renderer and terminal scope,
  project-scale boundary, warm/cold conditions, unexercised axes, and any
  missing input-to-photon, compositor, or human-usability evidence.
- Every admitted claim is phrased within those fixtures and limitations. A
  fixture-only result is never presented as a product-wide guarantee.
- The same fixture and limitations apply to both release candidates. A changed
  fixture is a new band and requires a new gate run.

Limitations document path: ______________________________________________

Fixture and excluded axes reviewed: ______________________________________

Evidence pointer: ______________________________________________

Status: `[ ] PASS`  `[ ] FAIL`

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Sign-off date (UTC): ______________________________________________

Reasoning or required follow-up: ______________________________________________

### Gate 6: Two-Release Regression

**Pass only when all of these are true:**

- Two consecutive release candidates run on the same host, container, project
  scale, and Ghostty baseline.
- Every measured class, including every claimed class, has a comparison row
  containing both candidate results and an explicit same-order-of-magnitude
  result. A missing class or a failed comparison refuses the target.
- STL SHA-256 is deterministic across the two runs. Record both hashes, not
  just a statement that they match.
- STEP and 3MF output comparison is documented. Record whether each format's
  bytes or hash diverged, why that divergence is expected or unexplained, and
  whether it limits the claim. Do not normalize a divergence away.
- Any regression drops the class back to a measured band until a later
  two-candidate run passes. A manual skip records the next measurement and
  does not extend an admitted target.

```text
Class comparison table: ______________________________________________
STL rc-1 SHA-256: __________________  STL rc-2 SHA-256: ______________
STL deterministic: [ ] YES  [ ] NO
STEP comparison and explanation: ______________________________________
3MF comparison and explanation: _______________________________________
```

Evidence pointer: ______________________________________________

Status: `[ ] PASS`  `[ ] FAIL`

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Sign-off date (UTC): ______________________________________________

Reasoning or required follow-up: ______________________________________________

## Current Pre-Release Run

The release script consumes the machine-readable record delimited below when
release material is selected with `THREETERM_RELEASE_MATERIAL`. Human prose in
this runbook is not evidence. Every signed record must bind its commit, tag,
evidence digest, owner, and admitted claim rows to the exact release.

<!-- PERFORMANCE-RECORD:START -->
record_status: UNSIGNED
release_commit: not recorded
release_tag: not recorded
evidence_path: not recorded
evidence_sha256: not recorded
owner: not recorded
record_signature: not recorded
record_date: not recorded
gate_1: FAIL
gate_1_signature: not recorded
gate_1_date: not recorded
gate_2: FAIL
gate_2_signature: not recorded
gate_2_date: not recorded
gate_3: FAIL
gate_3_signature: not recorded
gate_3_date: not recorded
gate_4: FAIL
gate_4_signature: not recorded
gate_4_date: not recorded
gate_5: FAIL
gate_5_signature: not recorded
gate_5_date: not recorded
gate_6: FAIL
gate_6_signature: not recorded
gate_6_date: not recorded
claim: none
hardware_profile: not recorded
project_scale: not recorded
limitations_path: not recorded
limitations_sha256: not recorded
stl_rc1_sha256: not recorded
stl_rc2_sha256: not recorded
stl_deterministic: NO
step_comparison: not recorded
three_mf_comparison: not recorded
<!-- PERFORMANCE-RECORD:END -->

Run date: **2026-08-26 UTC**

This run assesses the latest checked-in two-release-candidate artifact at
`docs/research/rehearsal-evidence/l-bracket/`. Its aggregate manifest SHA-256 is
`8e196d236a1973dbd08d0e67ab3537d171c7efe0936a085a33e1103afc033135`. The
artifact was produced in repository commit
`ab1151d2109cc640ca8056788cd206940f50022c`, identifies `rc-1` and `rc-2`, uses
the `nearest-rank` label, and reports `promoted: false`. The catalog verifier
passes for the checked-in files.

The artifact contains these nine timing classes:

```text
project_create, bracket_create, edit_open, edit_update, edit_preview,
edit_commit, reload, export, catalog
```

Each class has `sample_count: 1` in each release candidate. The aggregate
comparison table's nine rows report the same order of magnitude for all nine
classes, and the STL, STEP, and 3MF hashes are equal between the two checked-in
runs.
 However, the artifact does
not declare the required hardware/container/Ghostty profile, project-scale
counts, or a named limitations document; its one sample per class is below the
`n >= 30` percentile threshold, and it does not provide the required signed
interpretation of the STEP/3MF comparison. Catalog integrity therefore does
not promote these bands.

### Gate Assessment

| Gate | Result | Current-run evidence and reasoning |
| --- | --- | --- |
| 1. Rehearsal spike | `PASS` | Checked-in L-bracket root, two candidate runs, and catalog verification are present. |
| 2. Hardware profile | `FAIL` | CPU, memory, container, package, and Ghostty profile fields are not declared in the artifact. |
| 3. Project scale | `FAIL` | Feature, transaction, and `Derived Result` counts are not declared in the artifact. |
| 4. Statistical method | `FAIL` | The label is `nearest-rank`, but every class has `n: 1`, below the per-class `n >= 30` requirement. |
| 5. Fixture-vs-product limits | `FAIL` | No named limitations document is published with this artifact. |
| 6. Two-release regression | `FAIL` | Nine per-class comparisons and equal format hashes are present, but matching profile/scale and a signed STEP/3MF interpretation are not evidenced. |

Current-run gate owner: ______________________________________________

Current-run gate signature or initials: ______________________________________________

Current-run gate sign-off date (UTC): ______________________________________________

### Decision

**Bands admitted:** none.

**Bands refused:** `project_create`, `bracket_create`, `edit_open`,
`edit_update`, `edit_preview`, `edit_commit`, `reload`, `export`, and `catalog`.

**Reasoning:** Gate 1 passes for the checked-in L-bracket catalog, but Gates 2,
3, 4, and 5 fail because required evidence fields are absent. Gate 6 cannot
admit a target because the profile and scale cannot be shown to match and the
format comparison is not documented as a signed limitation. The timing values
remain internal measured bands only. The two release candidates must not be
pooled to turn `n: 1` into a qualifying sample count. No release note, marketing
statement, README claim, or per-class MVP target may use these values as an
admitted performance claim.

### Current Per-Class Decisions

| Class | Metric/unit | n per RC | Band or target | Evidence identity | Gate result | Decision | Reasoning |
| --- | --- | ---: | --- | --- | --- | --- | --- |
| `project_create` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `bracket_create` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `edit_open` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `edit_update` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `edit_preview` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `edit_commit` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `reload` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `export` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |
| `catalog` | timing / ms | 1 | measured band | aggregate manifest | Gates 2-6 fail | `REFUSE` | Missing profile, scale, limitations, and qualifying sample count. |

Owner name and role: ______________________________________________

Owner signature or initials: ______________________________________________

Decision date (UTC): ______________________________________________

## Future Per-Class Decision Record

Copy this table for every signed run. A blank, `REFUSED`, or unsigned row is not
an admitted target.

| Class | Metric/unit | n per RC | Band or target | Evidence identity | Gate result | Decision | Reasoning |
| --- | --- | ---: | --- | --- | --- | --- | --- |
|  |  |  |  |  |  | `ADMIT` / `REFUSE` |  |
|  |  |  |  |  |  | `ADMIT` / `REFUSE` |  |
|  |  |  |  |  |  | `ADMIT` / `REFUSE` |  |

## Final Publication Check

Before publishing a performance claim, the release owner must be able to check
all of these boxes:

- [ ] One current evidence artifact and immutable digest are attached.
- [ ] All six gate statuses are `PASS`.
- [ ] All six owner sign-off lines are complete and dated.
- [ ] Each published class is inside the declared fixture, profile, and scale.
- [ ] Each published percentile has `n >= 30` per class. Refused measured bands
  are omitted from public claims and targets.
- [ ] The admitted/refused decision table and reasoning are attached to the
  release record.
- [ ] STEP/3MF divergence and STL determinism are documented for the exact
  candidates being published.

If any box is unchecked, stop publication and return to the failing gate.
