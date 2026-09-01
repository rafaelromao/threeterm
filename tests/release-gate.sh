#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_SCRIPT="${ROOT}/.github/scripts/release.sh"
RUNBOOK="${ROOT}/docs/release/trademark-and-namespace-gate.md"

expect_failure() {
    if "$@" >/dev/null 2>&1; then
        printf 'expected failure: %s\n' "$*" >&2
        exit 1
    fi
}

expect_failure "${RELEASE_SCRIPT}" verify
tag="v0.0.0-release-gate-${BASHPID}"
expect_failure "${RELEASE_SCRIPT}" tag "${tag}"
if command -v git >/dev/null 2>&1 && git rev-parse --verify --quiet "refs/tags/${tag}" >/dev/null; then
    printf 'tag was created despite a failed release gate\n' >&2
    exit 1
fi
expect_failure "${RELEASE_SCRIPT}" github-release "${tag}"
expect_failure "${RELEASE_SCRIPT}" aur-push refs/heads/main
expect_failure "${RELEASE_SCRIPT}" copr-build threeterm.spec
expect_failure "${RELEASE_SCRIPT}" unknown-action
expect_failure "${RELEASE_SCRIPT}" tag
expect_failure "${RELEASE_SCRIPT}" tag --delete
expect_failure "${RELEASE_SCRIPT}" github-release "${tag}" extra
expect_failure "${RELEASE_SCRIPT}" aur-push
expect_failure "${RELEASE_SCRIPT}" copr-build invalid-package.txt

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT
fixture="${tmpdir}/signed.md"
candidate_date="$(date -u +%F)"
stale_date="$(date -u -d "${candidate_date} - 31 days" +%F)"
sed \
    -e 's/- \[ \]/- [x]/g' \
    -e 's/`not recorded`; signed: `not recorded`/`Rafael Romao`; signed: `2026-08-26`/g' \
    -e 's/`not set`; signed: `not recorded`/`Rafael Romao`; signed: `2026-08-26`/g' \
    -e 's/\[BLOCKED\]/[PASS]/g' \
    -e 's/`UNSIGNED`/`SIGNED`/g' \
    -e 's/`not authorized`/`APPROVED`/g' \
    -e 's/`not set`/`Rafael Romao`/g' \
    -e 's/query="not recorded"; source="not recorded"; result="not recorded"/query="ThreeTerm USPTO WIPO TMview EUIPO national Terminal Three .com crates.io 91298824 terminal-native downloadable branding release tag GitHub Release AUR push COPR build rehearsal"; source="https:\/\/tmsearch.uspto.gov\/; https:\/\/branddb.wipo.int\/; https:\/\/www.tmdn.org\/tmview\/; https:\/\/euipo.europa.eu\/; https:\/\/www.gov.uk\/; https:\/\/rdap.verisign.com\/com\/; https:\/\/crates.io\/; https:\/\/ttabvue.uspto.gov\/; docs\/research\/; .github\/scripts\/; repository"; result="No conflicting record; see recorded result"/g' \
    -e 's/list each office, URL, and exact\/similar query here/national office names, URLs, and exact\/similar results recorded/g' \
    -e 's/list each query\/result here/query\/result entries recorded/g' \
    -e 's/record exact lookups here/live RDAP lookup results recorded/g' \
    -e 's/record exact queries here/live registry query results recorded/g' \
    -e 's/inspect all public-facing uses/all public-facing uses inspected/g' \
    -e 's/not recorded/evidence captured/g' \
    -e "s|2026-08-25|${candidate_date}|g" \
    -e "s|2026-08-26|${candidate_date}|g" \
    "${RUNBOOK}" >"${fixture}"

"${RELEASE_SCRIPT}" verify "${fixture}"

release_root="${tmpdir}/release-root"
fake_bin="${tmpdir}/bin"
release_artifact="${tmpdir}/release-artifact"
mkdir -p "${release_root}" "${fake_bin}"
git archive HEAD | tar -x -C "${release_root}"
cp "${fixture}" "${release_root}/docs/release/trademark-and-namespace-gate.md"
git -C "${release_root}" init --quiet
git -C "${release_root}" config user.email ci@example.invalid
git -C "${release_root}" config user.name ci
git -C "${release_root}" add -A
git -C "${release_root}" commit --quiet -m signed-gate
current_tag="v0.0.0-release-gate-current-${BASHPID}"
old_tag="v0.0.0-release-gate-old-${BASHPID}"
git -C "${release_root}" tag -a "${old_tag}" -m old
printf '%s\n' source-change >>"${release_root}/README.md"
git -C "${release_root}" add README.md
git -C "${release_root}" commit --quiet -m source-change
record_commit="$(git -C "${release_root}" rev-parse HEAD)"
evidence_path="docs/research/rehearsal-evidence/l-bracket/sha256-manifest.json"
evidence_sha="$(sha256sum "${release_root}/${evidence_path}" | cut -d' ' -f1)"
limitations_path="docs/release/performance-claim-limitations.md"
limitations_sha="$(sha256sum "${release_root}/${limitations_path}" | cut -d' ' -f1)"
today="$(date -u +%F)"
awk -v commit="${record_commit}" -v tag="${current_tag}" -v evidence="${evidence_path}" \
    -v evidence_sha="${evidence_sha}" -v limitations="${limitations_path}" \
    -v limitations_sha="${limitations_sha}" -v today="${today}" '
    /<!-- PERFORMANCE-RECORD:START -->/ {
        print
        print "record_status: SIGNED"
        print "release_commit: " commit
        print "release_tag: " tag
        print "evidence_path: " evidence
        print "evidence_sha256: " evidence_sha
        print "hardware_profile: pinned-test-profile"
        print "project_scale: L-bracket-small"
        print "hardware_cpu: test-cpu"
        print "hardware_threads: 8"
        print "hardware_memory_mb: 16384"
        print "hardware_kernel: test-kernel"
        print "hardware_container: podman@test-image"
        print "hardware_ghostty: direct-local"
        print "fixture_name: L-bracket"
        print "feature_count: 1"
        print "transaction_count: 1"
        print "derived_result_count: 1"
        print "statistical_method: nearest-rank"
        print "units: ms"
        print "sample_minimum: 30"
        print "limitations_path: " limitations
        print "limitations_sha256: " limitations_sha
        print "stl_rc1_sha256: 0000000000000000000000000000000000000000000000000000000000000000"
        print "stl_rc2_sha256: 0000000000000000000000000000000000000000000000000000000000000000"
        print "stl_deterministic: YES"
        print "step_comparison: equal"
        print "step_comparison_explanation: deterministic-bytes"
        print "step_claim_impact: none"
        print "three_mf_comparison: equal"
        print "three_mf_comparison_explanation: deterministic-bytes"
        print "three_mf_claim_impact: none"
        print "owner: Release Owner"
        print "record_signature: Release Owner"
        print "record_date: " today
        for (i = 1; i <= 6; i++) {
            print "gate_" i ": PASS"
            print "gate_" i "_signature: Release Owner"
            print "gate_" i "_date: " today
        }
        print "claim: id=export metric=timing unit=ms percentile=p50 fixture=L-bracket scale=small n_rc1=30 n_rc2=30 decision=ADMIT"
        inside = 1
        next
    }
    /<!-- PERFORMANCE-RECORD:END -->/ { inside = 0; print; next }
    !inside { print }
' "${release_root}/docs/release/six-gate-performance-claims-gate.md" \
    >"${tmpdir}/signed-performance.md"
mv "${tmpdir}/signed-performance.md" \
    "${release_root}/docs/release/six-gate-performance-claims-gate.md"
git -C "${release_root}" add docs/release/six-gate-performance-claims-gate.md
git -C "${release_root}" commit --quiet -m performance-record
git -C "${release_root}" tag -a "${current_tag}" -m current
source "${ROOT}/.github/scripts/licensing.sh"
stage_libslvs_artifact "${release_root}" /bin/true "${release_artifact}" >/dev/null
cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >"${RELEASE_GATE_GH_ARGS}"
exit 0
EOF
chmod +x "${fake_bin}/gh"
verified_commit="$(git -C "${release_root}" rev-parse HEAD)"
trap 'rm -rf "${tmpdir}"' EXIT
PATH="${fake_bin}:${PATH}" RELEASE_GATE_CURRENT_TAG="${current_tag}" \
    RELEASE_GATE_OLD_TAG="${old_tag}" \
    RELEASE_GATE_COMMIT="${verified_commit}" \
    THREETERM_RELEASE_ARTIFACT_MANIFEST="${release_artifact}/manifest.json" \
    THREETERM_RELEASE_ARTIFACT_ROOT="${release_artifact}" \
    THREETERM_RELEASE_BUNDLE_ROOT="${release_root}/release-artifact" \
    RELEASE_GATE_GH_ARGS="${release_root}/gh-args" \
    "${release_root}/.github/scripts/release.sh" github-release "${current_tag}"
grep -Fq "threeterm-${current_tag}.tar.gz" "${release_root}/gh-args"
grep -Fq "release-manifest.json" "${release_root}/gh-args"
grep -Fq "SHA256SUMS" "${release_root}/gh-args"
grep -Fq "worker-manifest.json" "${release_root}/gh-args"

performance_material="${tmpdir}/unsupported-performance.md"
printf '%s\n' 'ThreeTerm performance claim: id=export metric=timing unit=ms percentile=p50 fixture=L-bracket scale=small' \
    >"${performance_material}"
PATH="${fake_bin}:${PATH}" RELEASE_GATE_GH_ARGS="${release_root}/gh-args" \
    THREETERM_RELEASE_MATERIAL="${performance_material}" \
    THREETERM_RELEASE_ARTIFACT_MANIFEST="${release_artifact}/manifest.json" \
    THREETERM_RELEASE_ARTIFACT_ROOT="${release_artifact}" \
    THREETERM_RELEASE_BUNDLE_ROOT="${release_root}/release-artifact" \
    "${release_root}/.github/scripts/release.sh" github-release "${current_tag}"
grep -Fq "unsupported-performance.md" "${release_root}/gh-args"
grep -Fq "six-gate-performance-claims-gate.md" "${release_root}/gh-args"
grep -Fq "sha256-manifest.json" "${release_root}/gh-args"

printf '%s\n' 'The export path is 2x faster.' >"${performance_material}"
rm -f "${release_root}/gh-args"
expect_failure env PATH="${fake_bin}:${PATH}" \
    RELEASE_GATE_CURRENT_TAG="${current_tag}" RELEASE_GATE_OLD_TAG="${old_tag}" \
    RELEASE_GATE_COMMIT="${verified_commit}" \
    RELEASE_GATE_GH_ARGS="${release_root}/gh-args" \
    THREETERM_RELEASE_MATERIAL="${performance_material}" \
    THREETERM_RELEASE_ARTIFACT_MANIFEST="${release_artifact}/manifest.json" \
    THREETERM_RELEASE_ARTIFACT_ROOT="${release_artifact}" \
    THREETERM_RELEASE_BUNDLE_ROOT="${release_root}/release-artifact" \
    "${release_root}/.github/scripts/release.sh" github-release "${current_tag}"
if [[ -e "${release_root}/gh-args" ]]; then
    printf '%s\n' 'GitHub API was called for an unsupported performance claim' >&2
    exit 1
fi
expect_failure env PATH="${fake_bin}:${PATH}" \
    RELEASE_GATE_CURRENT_TAG="${current_tag}" RELEASE_GATE_OLD_TAG="${old_tag}" \
    RELEASE_GATE_COMMIT="${verified_commit}" \
    THREETERM_RELEASE_ARTIFACT_MANIFEST="${release_artifact}/manifest.json" \
    THREETERM_RELEASE_ARTIFACT_ROOT="${release_artifact}" \
    THREETERM_RELEASE_BUNDLE_ROOT="${release_root}/release-artifact" \
    "${release_root}/.github/scripts/release.sh" github-release "${old_tag}"

stale_fixture="${tmpdir}/stale.md"
sed "s|Evidence date: \`${candidate_date}\`|Evidence date: \`${stale_date}\`|g" \
    "${fixture}" >"${stale_fixture}"
expect_failure "${RELEASE_SCRIPT}" verify "${stale_fixture}"

inconsistent_fixture="${tmpdir}/inconsistent.md"
sed '0,/Product-owner sign-off: `Rafael Romao`/s//Product-owner sign-off: `Another Owner`/' \
    "${fixture}" >"${inconsistent_fixture}"
expect_failure "${RELEASE_SCRIPT}" verify "${inconsistent_fixture}"

placeholder_fixture="${tmpdir}/placeholder.md"
sed \
    -e 's/query="ThreeTerm USPTO[^\"]*"/query="live check"/' \
    -e 's#source="https://tmsearch.uspto.gov/[^\"]*"#source="https://example.com/placeholder"#' \
    "${fixture}" >"${placeholder_fixture}"
expect_failure "${RELEASE_SCRIPT}" verify "${placeholder_fixture}"

printf 'release gate tests passed\n'
