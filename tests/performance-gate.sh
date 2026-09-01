#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

source "${ROOT}/.github/scripts/performance-gate.sh"
commit="$(git rev-parse HEAD)"
today="$(date -u +%F)"
evidence="${ROOT}/docs/research/rehearsal-evidence/l-bracket/sha256-manifest.json"
evidence_sha="$(sha256sum "${evidence}" | cut -d' ' -f1)"
limitations="${ROOT}/docs/release/performance-claim-limitations.md"
limitations_sha="$(sha256sum "${limitations}" | cut -d' ' -f1)"
record="${WORK}/six-gate.md"
cat >"${record}" <<EOF
<!-- PERFORMANCE-RECORD:START -->
record_status: SIGNED
release_commit: ${commit}
release_tag: v0.1.0-performance
evidence_path: docs/research/rehearsal-evidence/l-bracket/sha256-manifest.json
evidence_sha256: ${evidence_sha}
hardware_profile: pinned-test-profile
project_scale: L-bracket-small
limitations_path: docs/release/performance-claim-limitations.md
limitations_sha256: ${limitations_sha}
stl_rc1_sha256: 0000000000000000000000000000000000000000000000000000000000000000
stl_rc2_sha256: 0000000000000000000000000000000000000000000000000000000000000000
stl_deterministic: YES
step_comparison: documented
three_mf_comparison: documented
owner: Release Owner
record_signature: Release Owner
record_date: ${today}
gate_1: PASS
gate_1_signature: Release Owner
gate_1_date: ${today}
gate_2: PASS
gate_2_signature: Release Owner
gate_2_date: ${today}
gate_3: PASS
gate_3_signature: Release Owner
gate_3_date: ${today}
gate_4: PASS
gate_4_signature: Release Owner
gate_4_date: ${today}
gate_5: PASS
gate_5_signature: Release Owner
gate_5_date: ${today}
gate_6: PASS
gate_6_signature: Release Owner
gate_6_date: ${today}
claim: id=export metric=timing unit=ms percentile=p50 fixture=L-bracket scale=small n_rc1=30 n_rc2=30 decision=ADMIT
<!-- PERFORMANCE-RECORD:END -->
EOF
material="${WORK}/notes.md"
printf '%s\n' 'ThreeTerm performance claim: id=export metric=timing unit=ms percentile=p50 fixture=L-bracket scale=small' >"${material}"

verify_performance_material "${ROOT}" "${material}" "${record}" "${commit}" "v0.1.0-performance"

if verify_performance_material "${ROOT}" "${material}" "${record}" "${commit}" "v0.1.0-other" >/dev/null 2>&1; then
    printf '%s\n' 'performance record with wrong tag was accepted' >&2
    exit 1
fi
if verify_performance_material "${ROOT}" "${material}" "${ROOT}/docs/release/six-gate-performance-claims-gate.md" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'unsigned current performance record was accepted' >&2
    exit 1
fi
printf '%s\n' 'ThreeTerm performance claim: export completed' >"${material}"
if verify_performance_material "${ROOT}" "${material}" "${record}" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'unscoped performance claim was accepted' >&2
    exit 1
fi
printf '%s\n' 'The export path is 2x faster.' >"${material}"
if verify_performance_material "${ROOT}" "${material}" "${record}" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'comparative performance claim was accepted' >&2
    exit 1
fi

printf '%s\n' 'performance claims gate contract satisfied'
