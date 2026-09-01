#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

source "${ROOT}/.github/scripts/performance-gate.sh"
source_root="${WORK}/source"
mkdir -p "${source_root}/docs/research/rehearsal-evidence" "${source_root}/docs/release"
cp -a "${ROOT}/docs/research/rehearsal-evidence/l-bracket" \
    "${source_root}/docs/research/rehearsal-evidence/"
cp "${ROOT}/docs/release/performance-claim-limitations.md" "${source_root}/docs/release/"
evidence_file="${source_root}/docs/research/rehearsal-evidence/l-bracket/sha256-manifest.json"
jq '.hardware_profile = "pinned-test-profile" | .project_scale = "small" |
    .hardware_cpu = "test-cpu" | .hardware_threads = 8 | .hardware_memory_mb = 16384 |
    .hardware_kernel = "test-kernel" | .hardware_microcode = "test-microcode" |
    .hardware_container = "podman@test-image" |
    .hardware_container_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000" |
    .hardware_package_versions = "rust=1.97.1,occt=pinned,libslvs=pinned,lib3mf=pinned" | .hardware_toolchain = "rust-1.97.1" |
    .hardware_ghostty = "xterm-ghostty/1.3.1" | .hardware_term = "xterm-ghostty" |
    .hardware_term_program = "ghostty" | .hardware_topology = "direct-local" |
    .feature_count = 1 | .transaction_count = 1 | .derived_result_count = 1 |
    (.runs[].timings[].sample_count) = 30 |
    (.runs[].timings[].samples_ms) = ([range(0; 30) | . + 1.0]) |
    (.runs[].timings[].p50_ms) = 15.0 | (.runs[].timings[].p95_ms) = 29.0 |
    (.runs[].timings[].p99_ms) = 30.0' \
    "${evidence_file}" >"${WORK}/evidence.json"
mv "${WORK}/evidence.json" "${evidence_file}"
git -C "${source_root}" init --quiet
git -C "${source_root}" config user.email ci@example.invalid
git -C "${source_root}" config user.name ci
git -C "${source_root}" add -A
git -C "${source_root}" commit --quiet -m evidence
record_commit="$(git -C "${source_root}" rev-parse HEAD)"
jq --arg commit "${record_commit}" '.source_commit = $commit' "${evidence_file}" >"${WORK}/evidence.json"
mv "${WORK}/evidence.json" "${evidence_file}"
git -C "${source_root}" add "${evidence_file}"
git -C "${source_root}" commit --quiet -m source-identity
commit="$(git -C "${source_root}" rev-parse HEAD)"
today="$(date -u +%F)"
evidence="${source_root}/docs/research/rehearsal-evidence/l-bracket/sha256-manifest.json"
evidence_sha="$(sha256sum "${evidence}" | cut -d' ' -f1)"
limitations="${source_root}/docs/release/performance-claim-limitations.md"
limitations_sha="$(sha256sum "${limitations}" | cut -d' ' -f1)"
stl_rc1="$(jq -er '.runs[] | select(.release_candidate == "rc-1") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.stl")) | .sha256' "${evidence}")"
stl_rc2="$(jq -er '.runs[] | select(.release_candidate == "rc-2") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.stl")) | .sha256' "${evidence}")"
step_rc1="$(jq -er '.runs[] | select(.release_candidate == "rc-1") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.step")) | .sha256' "${evidence}")"
step_rc2="$(jq -er '.runs[] | select(.release_candidate == "rc-2") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.step")) | .sha256' "${evidence}")"
three_mf_rc1="$(jq -er '.runs[] | select(.release_candidate == "rc-1") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.3mf")) | .sha256' "${evidence}")"
three_mf_rc2="$(jq -er '.runs[] | select(.release_candidate == "rc-2") | .artifacts[] | select(.relative_path | endswith("/export/l-bracket.3mf")) | .sha256' "${evidence}")"
record="${WORK}/six-gate.md"
cat >"${record}" <<EOF
<!-- PERFORMANCE-RECORD:START -->
record_status: SIGNED
release_commit: ${commit}
release_tag: v0.1.0-performance
evidence_path: docs/research/rehearsal-evidence/l-bracket/sha256-manifest.json
evidence_sha256: ${evidence_sha}
hardware_profile: pinned-test-profile
project_scale: small
hardware_cpu: test-cpu
hardware_threads: 8
hardware_memory_mb: 16384
hardware_kernel: test-kernel
hardware_microcode: test-microcode
hardware_container: podman@test-image
hardware_container_digest: sha256:0000000000000000000000000000000000000000000000000000000000000000
hardware_package_versions: rust=1.97.1,occt=pinned,libslvs=pinned,lib3mf=pinned
hardware_toolchain: rust-1.97.1
hardware_ghostty: xterm-ghostty/1.3.1
hardware_term: xterm-ghostty
hardware_term_program: ghostty
hardware_topology: direct-local
fixture_name: L-bracket
feature_count: 1
transaction_count: 1
derived_result_count: 1
statistical_method: nearest-rank
units: ms
sample_minimum: 30
independent_sample_definition: one-process-run-per-sample
limitations_path: docs/release/performance-claim-limitations.md
limitations_sha256: ${limitations_sha}
stl_rc1_sha256: ${stl_rc1}
stl_rc2_sha256: ${stl_rc2}
step_rc1_sha256: ${step_rc1}
step_rc2_sha256: ${step_rc2}
3mf_rc1_sha256: ${three_mf_rc1}
3mf_rc2_sha256: ${three_mf_rc2}
stl_deterministic: YES
step_comparison: equal
step_comparison_explanation: deterministic-bytes
step_claim_impact: none
three_mf_comparison: equal
three_mf_comparison_explanation: deterministic-bytes
three_mf_claim_impact: none
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
while IFS=$'\t' read -r class rc1 rc2; do
    printf 'comparison: class=%s rc1=%s rc2=%s same_order=YES\n' "$class" "$rc1" "$rc2"
done < <(jq -r '.comparisons[] | [.class, .run_1.p50_ms, .run_2.p50_ms] | @tsv' "${evidence}") \
    >"${WORK}/comparisons"
awk -v comparisons="${WORK}/comparisons" '/^claim: / { while ((getline line < comparisons) > 0) print line } { print }' \
    "${record}" >"${WORK}/record-with-comparisons"
mv "${WORK}/record-with-comparisons" "${record}"
git -C "${source_root}" commit --quiet --allow-empty -m release-record
commit="$(git -C "${source_root}" rev-parse HEAD)"
material="${WORK}/notes.md"
printf '%s\n' 'ThreeTerm performance claim: id=export metric=timing unit=ms percentile=p50 fixture=L-bracket scale=small' >"${material}"

verify_performance_material "${source_root}" "${material}" "${record}" "${commit}" "v0.1.0-performance"

conflicting_record="${WORK}/conflicting-record.md"
sed '/^claim: id=export /a claim: id=export metric=timing unit=ms percentile=p95 fixture=L-bracket scale=small n_rc1=30 n_rc2=30 decision=ADMIT' \
    "${record}" >"${conflicting_record}"
if verify_performance_material "${source_root}" "${material}" "${conflicting_record}" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'conflicting performance claim identity was accepted' >&2
    exit 1
fi

git -C "${source_root}" commit --quiet --allow-empty -m newer-source
new_commit="$(git -C "${source_root}" rev-parse HEAD)"
if verify_performance_material "${source_root}" "${material}" "${record}" "${new_commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'stale performance record was accepted for a newer source commit' >&2
    exit 1
fi

if verify_performance_material "${source_root}" "${material}" "${record}" "${commit}" "v0.1.0-other" >/dev/null 2>&1; then
    printf '%s\n' 'performance record with wrong tag was accepted' >&2
    exit 1
fi
if verify_performance_material "${source_root}" "${material}" "${ROOT}/docs/release/six-gate-performance-claims-gate.md" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'unsigned current performance record was accepted' >&2
    exit 1
fi
printf '%s\n' 'ThreeTerm performance claim: export completed' >"${material}"
if verify_performance_material "${source_root}" "${material}" "${record}" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'unscoped performance claim was accepted' >&2
    exit 1
fi
printf '%s\n' 'The export path is 2x faster.' >"${material}"
if verify_performance_material "${source_root}" "${material}" "${record}" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'comparative performance claim was accepted' >&2
    exit 1
fi
printf '%s\n' 'The export is fast.' >"${material}"
if verify_performance_material "${source_root}" "${material}" "${record}" "${commit}" "v0.1.0-performance" >/dev/null 2>&1; then
    printf '%s\n' 'unstructured fast claim was accepted' >&2
    exit 1
fi

printf '%s\n' 'performance claims gate contract satisfied'
