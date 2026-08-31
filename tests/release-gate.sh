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
source "${ROOT}/.github/scripts/licensing.sh"
stage_libslvs_artifact "${ROOT}" /bin/true "${release_artifact}" >/dev/null
mkdir -p "${release_root}/.github/scripts" "${release_root}/docs/release" "${fake_bin}"
cp "${RELEASE_SCRIPT}" "${release_root}/.github/scripts/release.sh"
cp "${ROOT}/.github/scripts/licensing.sh" "${release_root}/.github/scripts/licensing.sh"
cp "${fixture}" "${release_root}/docs/release/trademark-and-namespace-gate.md"
cat >"${fake_bin}/git" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == status ]]; then
    exit 0
fi
if [[ "${1:-}" == get-tar-commit-id ]]; then
    /usr/bin/git "$@"
    exit $?
fi
if [[ "${1:-}" == rev-parse ]]; then
    if [[ "$*" == *"${RELEASE_GATE_OLD_TAG}^{commit}"* ]]; then
        printf 'old-commit\n'
    else
        printf 'current-commit\n'
    fi
    exit 0
fi
exit 1
EOF
cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "${fake_bin}/git" "${fake_bin}/gh"

current_tag="v0.0.0-release-gate-current-${BASHPID}"
old_tag="v0.0.0-release-gate-old-${BASHPID}"
trap 'rm -rf "${tmpdir}"' EXIT
PATH="${fake_bin}:${PATH}" RELEASE_GATE_CURRENT_TAG="${current_tag}" \
    RELEASE_GATE_OLD_TAG="${old_tag}" \
    THREETERM_RELEASE_ARTIFACT_MANIFEST="${release_artifact}/manifest.json" \
    THREETERM_RELEASE_ARTIFACT_ROOT="${release_artifact}" \
    "${release_root}/.github/scripts/release.sh" github-release "${current_tag}"
expect_failure env PATH="${fake_bin}:${PATH}" \
    RELEASE_GATE_CURRENT_TAG="${current_tag}" RELEASE_GATE_OLD_TAG="${old_tag}" \
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
