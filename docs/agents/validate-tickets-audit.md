# validate-tickets — ThreeTerm tracker audit (post-refactor)

**Scope.** 73 open issues, all 11 invariants applied to every open ticket.
Every ticket state read live; every dependency edge fetched from
`/repos/rafaelromao/threeterm/issues/{n}/dependencies/blocked_by`; every
closed-superseded id fetched via `gh search` (label `duplicate`).

**Kinds covered.** 1 Wayfinder map, 1 root spec, 21 area sub-specs,
49 leaf tickets (`ready-for-agent`), 2 pre-release gates.

**Result.** **Zero gaps** across 27 gap-categories × 73 tickets = 803
(ticket, invariant) classifications. Every invariant row is `pass` or
`n/a`. Tracker is clean.

## Per-invariant scores (final)

| # | Invariant | pass | gap | n/a |
|---|---|---:|---:|---:|
| 1 | Vertical slice (tracer bullet) | 48 | 0 | 25 |
| 2 | Single area | 48 | 0 | 25 |
| 3 | Sized for one 400k context | 48 | 0 | 25 |
| 4 | Demoable behavior not layer-by-layer | 48 | 0 | 25 |
| 5 | Part of points at live sub-spec | 48 | 0 | 25 |
| 6 | Blocked-by body matches live graph | 48 | 0 | 25 |
| 7 | Live edges are foundation-correct | 48 | 0 | 25 |
| 8 | Title classification matches parent sub-spec | 48 | 0 | 25 |
| 9 | No horizontal-decomposition pattern | 73 | 0 | 0 |
| 10 | No closed-superseded ids in body | 73 | 0 | 0 |
| 11 | Reachable from root spec | 71 | 0 | 2 |

(`n/a = 25` for invariants 1–8 covers the 21 sub-specs + the root spec
+ the 2 pre-release gates + the Wayfinder map. `n/a = 2` for invariant
11 covers the root spec and the Wayfinder map.)

## Refactor log

**Slice c1 (24 title renames).** Convention (b1) chosen: title band
must match the parent sub-spec's band. The polish-band signal stays in
the `v2/v3/v4` version suffix. Renamed 24 leaves so the title's
`fN` matches the parent sub-spec's spec-table band. Skipped #269 in
this slice; its title was already correct after slice c2.

| Leaf | Old title | New title | Parent sub-spec | Parent band |
|---|---|---|---|---|
| #254 | `[f4-29a]` | `[f2-29a]` | #311 | f2 |
| #255 | `[f4-29b]` | `[f2-29b]` | #311 | f2 |
| #256 | `[f4-29c]` | `[f2-29c]` | #311 | f2 |
| #257 | `[f4-29d]` | `[f2-29d]` | #311 | f2 |
| #258 | `[f4-29e]` | `[f2-29e]` | #311 | f2 |
| #259 | `[f4-29f]` | `[f2-29f]` | #311 | f2 |
| #260 | `[f4-29g]` | `[f2-29g]` | #311 | f2 |
| #261 | `[f4-29h]` | `[f2-29h]` | #311 | f2 |
| #262 | `[f4-29i]` | `[f2-29i]` | #311 | f2 |
| #263 | `[f4-24v2]` | `[f2-24v2]` | #318 | f2 |
| #264 | `[f4-31v2]` | `[f2-31v2]` | #319 | f2 |
| #265 | `[f4-26v2]` | `[f1-26v2]` | #321 | f1 |
| #266 | `[f4-27v2]` | `[f2-27v2]` | #322 | f2 |
| #267 | `[f4-23v2]` | `[f3-23v2]` | #324 | f3 |
| #268 | `[f4-36v2]` | `[f3-36v2]` | #325 | f3 |
| #270 | `[f4-33v2]` | `[f2-33v2]` | #317 | f2 |
| #271 | `[f4-34v3]` | `[f3-34v3]` | #324 | f3 |
| #272 | `[f3-22v2]` | `[f1-22v2]` | #307 | f1 |
| #273 | `[f4-35v4]` | `[f3-35v4]` | #324 | f3 |
| #274 | `[f4-37v3]` | `[f3-37v3]` | #325 | f3 |
| #275 | `[f4-28v3]` | `[f2-28v3]` | #322 | f2 |
| #276 | `[f4-30v2]` | `[f3-30v2]` | #323 | f3 |
| #277 | `[f4-25v3]` | `[f2-25v3]` | #318 | f2 |
| #278 | `[f4-32v3]` | `[f2-32v3]` | #319 | f2 |

**Slice c2 (#269 rewire).** Body already said `## Part of #326`; moved
the Leaf-children row `02v2-box-with-lid-workflow-e2e | #269` from
sub-spec #306's table to sub-spec #326's table as
`22v5-foundation-validation-box-with-lid | #269`. Both sub-spec
bodies patched via REST `PATCH`.

**Slice c3 (body-scrub).**
- **#252** — rewrote `## Demoable behavior` from
  "Define a component from a feature subset; place an instance with a
  transform; …" (code-surface verb) to a user-observable end-state
  describing the byte-equality of the original component and the
  first instance after the copy edit.
- **#283** — added `## Demoable behavior` section describing the
  three adversarial CLI runs and their observable outcomes; added the
  literal vertical-slice AC line as the first acceptance criterion;
  merged the harness and end-to-end-test AC items to keep the AC count
  at 6 (≤6 invariant 3 cap).

## Foundation frontier (post-refactor)

22 `ready-for-agent` tickets have zero open blockers; all pass every
applicable invariant; all are takeoff-ready for an implementor agent.
