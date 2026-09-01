# ThreeTerm Trademark and Namespace Release Gate

This is the single source of truth for the product-owner gate before any
ThreeTerm public release. It implements user story 58 in the [ThreeTerm MVP
specification (#58)](https://github.com/rafaelromao/threeterm/issues/58).

The owner must complete the **Current release gate** in this file and commit
the signed result before using `.github/scripts/release.sh`. The script refuses
an unsigned, stale, incomplete, or blocked gate. Do not cut a tag, create a
GitHub Release, push the AUR package, or submit a COPR build directly.

The use conditions and similar-mark analysis come from the closed Wayfinder
decision [Complete ThreeTerm trademark and namespace clearance (#55)](https://github.com/rafaelromao/threeterm/issues/55), with the namespace gate from
[Validate ThreeTerm release namespaces (#53)](https://github.com/rafaelromao/threeterm/issues/53)
and the collision decision from [Decide whether to accept ThreeTerm's
terminal-software collision (#54)](https://github.com/rafaelromao/threeterm/issues/54).
This document is an operational clearance gate, not legal advice or a legal
opinion.

## How to run the gate

1. Set the candidate date to the day the release will be authorized.
2. Run live professional searches in USPTO, WIPO, TMview, EUIPO, and each
   relevant national office, plus every availability check, again. Record the
   exact query, source URL, result, and date in the current gate. Evidence must
   be no more than 30 days old on the candidate date; a historical result is
   not a fresh check.
3. Reassess opposition `91298824` against the planned use and record the
   conclusion, not just a docket lookup.
4. Confirm each use condition and publication guardrail. Use `[PASS]` only
   when the evidence supports release; use `[ACCEPTED]` only for a documented
   risk that the product owner explicitly accepts.
5. Replace every `not recorded` value with the same product-owner identity and
   candidate date. Set the gate status to `SIGNED` and release authorization to
   `APPROVED` only after every row is complete.
6. Run `.github/scripts/release.sh verify`. Only then invoke one of its fixed
   action commands.

## Current release gate

The checked-in gate intentionally starts unsigned and blocked. The product
owner must update this section in place for each release; do not edit the
historical baseline below to make a release pass.

<!-- CURRENT-GATE:START -->
Current gate status: `UNSIGNED`
Candidate date: `2026-08-26`
Declared product owner: `not set`
Product-owner authorization date: `2026-08-26`
Product-owner authorization: `not set`; signed: `not recorded`
Release authorization: `not authorized`
Evidence freshness window: `30 days`

- [ ] **T-USPTO** Search `ThreeTerm` and the close variants in the USPTO trademark database, including live similar-mark results.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `USPTO Trademark Search https://tmsearch.uspto.gov/; exact and similar queries`
  - Disposition: `[BLOCKED] fresh professional search not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **T-WIPO** Search `ThreeTerm` and the close variants in the WIPO Global Brand Database for intended markets.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `WIPO Global Brand Database https://branddb.wipo.int/; exact and similar queries`
  - Disposition: `[BLOCKED] fresh professional search not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **T-TMVIEW** Search `ThreeTerm` and the close variants in TMview across relevant participating offices.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `TMview https://www.tmdn.org/tmview/; exact and similar queries`
  - Disposition: `[BLOCKED] fresh professional search not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **T-EUIPO** Search `ThreeTerm` and the close variants in EUIPO records for the intended goods and services.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `EUIPO eSearch plus https://euipo.europa.eu/; exact and similar queries`
  - Disposition: `[BLOCKED] fresh professional search not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **T-NATIONAL** Search relevant national offices for every intended market, recording the office and query rather than relying on an aggregator.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `relevant national offices; list each office, URL, and exact/similar query here`
  - Disposition: `[BLOCKED] relevant national offices not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **T-VARIANTS** Search each of `ThreeTerm`, `3Term`, `Terminal3`, `Terminal Three`, and `Terminal 3` in every applicable trademark source, including spelling, phonetic, and conceptual similarities.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `USPTO, WIPO, TMview, EUIPO, and relevant national offices; list each query/result here`
  - Disposition: `[BLOCKED] close-variant analysis not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **D-DOMAINS** Perform a fresh availability and registration check for `threeterm.com`, `threeterm.app`, `threeterm.dev`, `threeterm.io`, and all project-specific TLDs.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `registrar/RDAP sources; .com, .app, .dev, .io, and project-specific TLDs; record exact lookups here`
  - Disposition: `[BLOCKED] domain checks not refreshed`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **P-PACKAGES** Perform a fresh namespace check for crates.io, npm, PyPI, AUR, Homebrew, and every relevant package or distribution channel.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `crates.io, npm, PyPI, AUR, Homebrew, and relevant package/distribution channels; record exact queries here`
  - Disposition: `[BLOCKED] package namespace checks not refreshed`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **O-OPPOSITION** Reassess pending USPTO opposition `TERMINAL3`, proceeding `91298824`, against the planned ThreeTerm use, goods/services, markets, and collision risk.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `USPTO proceeding 91298824 https://ttabvue.uspto.gov/; planned-use comparison and current status`
  - Disposition: `[BLOCKED] opposition reassessment not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **U-TERM** Confirm `ThreeTerm` is paired with terminal-native parametric CAD terminology in product, documentation, and release copy.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `planned product, documentation, and release copy; inspect all public-facing uses`
  - Disposition: `[BLOCKED] use-condition confirmation not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **U-SCOPE** Confirm initial use is limited to downloadable or open-source CAD software and does not imply broader goods or services.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `release metadata, download pages, source repository, and planned notices`
  - Disposition: `[BLOCKED] scope confirmation not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **U-BRANDING** Confirm `3Term`, `Terminal3`, `Terminal Three`, and `Terminal 3` are forbidden in branding, release names, package names, and product-facing copy.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `repository, release metadata, package manifests, distribution recipes, and public copy`
  - Disposition: `[BLOCKED] branding prohibition not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **E-REHEARSAL** Compare the release candidate with the latest published rehearsal evidence and record its date, sources, and item-by-item disposition.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `docs/research/rehearsal-evidence/README.md; committed L-bracket run-1, run-2, and adversarial catalogs`
  - Disposition: `[BLOCKED] rehearsal comparison and clearance disposition not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **G-TAG** Confirm the signed gate is committed before creating the release tag.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `.github/scripts/release.sh tag; signed runbook commit and candidate tag`
  - Disposition: `[BLOCKED] release tag authorization not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **G-GITHUB** Confirm the signed gate is verified immediately before creating the GitHub Release.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `.github/scripts/release.sh github-release; GitHub Release target and assets`
  - Disposition: `[BLOCKED] GitHub Release authorization not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **G-AUR** Confirm the signed gate is verified immediately before the AUR push.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `.github/scripts/release.sh aur-push; AUR package metadata and remote`
  - Disposition: `[BLOCKED] AUR push authorization not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
- [ ] **G-COPR** Confirm the signed gate is verified immediately before the COPR build.
  - Evidence date: `2026-08-25`
  - Evidence record: `query="not recorded"; source="not recorded"; result="not recorded"`
  - Sources consulted: `.github/scripts/release.sh copr-build; COPR project and spec metadata`
  - Disposition: `[BLOCKED] COPR build authorization not recorded`
  - Product-owner sign-off: `not recorded`; signed: `not recorded`
<!-- CURRENT-GATE:END -->

## Historical rehearsal baseline

This is the latest committed rehearsal evidence available when this gate was
created. It is recorded for auditability and is **not** a trademark or
namespace clearance result. The namespace research explicitly says that its
automated screening is not legal clearance and that the earlier exact-name
terminal project remains relevant searchability evidence.

- Evidence set: `docs/research/rehearsal-evidence/l-bracket/`
- Published evidence commit: `fa31f0b` (`2026-08-25`), including two consecutive release-candidate runs and adversarial cases.
- Namespace-screening reference: `docs/research/threeterm-release-namespace-validation.md`, checked `2026-07-30`.

| Evidence item | Date | Sources consulted | Disposition |
|---|---|---|---|
| L-bracket release-candidate run 1 | 2026-08-25 | committed SHA-256 catalog and canonical project/export files | PASS for rehearsal integrity only; no clearance conclusion |
| L-bracket release-candidate run 2 | 2026-08-25 | committed SHA-256 catalog and canonical project/export files | PASS for consecutive-run evidence only; no clearance conclusion |
| L-bracket adversarial cases | 2026-08-25 | committed adversarial reports and SHA-256 catalog | PASS for adversarial validation only; no clearance conclusion |
| Exact-name and namespace screening | 2026-07-30 | USPTO guidance, GitHub, package registries, Linux distributions, RDAP sources, WIPO/TMview guidance listed in the research document | NOT CLEARED; professional/live owner search still required |

## Release commands

These commands are intentionally narrow. They verify the checked-in runbook
before performing the action and do not accept a runbook override:

```text
.github/scripts/release.sh verify
.github/scripts/release.sh build <annotated-tag>
.github/scripts/release.sh tag <annotated-tag>
.github/scripts/release.sh github-release <tag>
.github/scripts/release.sh aur-push HEAD:refs/heads/master
.github/scripts/release.sh copr-build threeterm.spec [--nowait]
```

The canonical release script is the only supported release entry point. Its
four publication commands fail closed when any current-gate item is unchecked,
unsigned, stale, blocked, or not explicitly authorized.

Set `THREETERM_RELEASE_MATERIAL` to the exact release notes or publication
material before `build`, `tag`, `github-release`, `aur-push`, or `copr-build`.
If that material contains a performance claim, the script verifies the signed
machine-readable record in `docs/release/six-gate-performance-claims-gate.md`
before doing any release side effect. GitHub Release attaches the material,
deterministic archive, release manifest, checksum catalog, and worker manifest.
