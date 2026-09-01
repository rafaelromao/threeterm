#!/usr/bin/env bash
# Canonical ThreeTerm release entry point. Every public artifact is gated by
# the signed current section of docs/release/trademark-and-namespace-gate.md.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RUNBOOK="${ROOT}/docs/release/trademark-and-namespace-gate.md"
LICENSING_SCRIPT="${ROOT}/.github/scripts/licensing.sh"
RELEASE_ARTIFACTS_SCRIPT="${ROOT}/.github/scripts/release-artifacts.sh"
PERFORMANCE_GATE_SCRIPT="${ROOT}/.github/scripts/performance-gate.sh"
readonly ROOT RUNBOOK

# shellcheck source=/dev/null
source "$RELEASE_ARTIFACTS_SCRIPT"
# shellcheck source=/dev/null
source "$PERFORMANCE_GATE_SCRIPT"
# shellcheck source=/dev/null
source "$LICENSING_SCRIPT"

readonly -a REQUIRED_ROWS=(
    T-USPTO T-WIPO T-TMVIEW T-EUIPO T-NATIONAL T-VARIANTS
    D-DOMAINS P-PACKAGES O-OPPOSITION U-TERM U-SCOPE U-BRANDING
    E-REHEARSAL G-TAG G-GITHUB G-AUR G-COPR
)

declare -A REQUIRED_QUERY_TOKENS=(
    [T-USPTO]='USPTO'
    [T-WIPO]='WIPO'
    [T-TMVIEW]='TMview'
    [T-EUIPO]='EUIPO'
    [T-NATIONAL]='national'
    [T-VARIANTS]='Terminal Three'
    [D-DOMAINS]='.com'
    [P-PACKAGES]='crates.io'
    [O-OPPOSITION]='91298824'
    [U-TERM]='terminal-native'
    [U-SCOPE]='downloadable'
    [U-BRANDING]='branding'
    [E-REHEARSAL]='rehearsal'
    [G-TAG]='release tag'
    [G-GITHUB]='GitHub Release'
    [G-AUR]='AUR push'
    [G-COPR]='COPR build'
)

declare -A REQUIRED_SOURCE_TOKENS=(
    [T-USPTO]='tmsearch.uspto.gov'
    [T-WIPO]='branddb.wipo.int'
    [T-TMVIEW]='tmdn.org/tmview'
    [T-EUIPO]='euipo.europa.eu'
    [T-NATIONAL]='national'
    [T-VARIANTS]='tmsearch.uspto.gov'
    [D-DOMAINS]='rdap.verisign.com'
    [P-PACKAGES]='crates.io'
    [O-OPPOSITION]='ttabvue.uspto.gov'
    [U-TERM]='docs/'
    [U-SCOPE]='release'
    [U-BRANDING]='repository'
    [E-REHEARSAL]='rehearsal'
    [G-TAG]='.github/scripts/'
    [G-GITHUB]='.github/scripts/'
    [G-AUR]='.github/scripts/'
    [G-COPR]='.github/scripts/'
)

fail() {
    printf 'release gate: %s\n' "$1" >&2
    exit 1
}

verify_release_artifact() {
    local artifact_root manifest
    artifact_root="$(release_artifact_root)"
    manifest="$(release_artifact_manifest "$artifact_root")"
    [[ "$(realpath -m "$manifest")" == "$(realpath -m "$artifact_root/manifest.json")" ]] \
        || fail 'release artifact manifest must be the manifest inside the artifact root'
    [[ -f "$LICENSING_SCRIPT" ]] || fail "licensing verifier not found: ${LICENSING_SCRIPT}"
    # shellcheck source=/dev/null
    source "$LICENSING_SCRIPT"
    verify_libslvs_artifact "$manifest" "$artifact_root"
}

release_source_root() {
    printf '%s\n' "${ROOT}"
}

release_bundle_root() {
    printf '%s\n' "${THREETERM_RELEASE_BUNDLE_ROOT:-${ROOT}/target/release-artifact}"
}

release_verified_commit() {
    git -C "$(release_source_root)" rev-parse HEAD
}

verify_release_material() {
    verify_performance_material \
        "$(release_source_root)" \
        "${THREETERM_RELEASE_MATERIAL:-}" \
        "${ROOT}/docs/release/six-gate-performance-claims-gate.md" \
        "$1" "$2"
}

build_release_bundle_for() {
    local tag="$1"
    local commit="$2"
    build_release_bundle \
        "$(release_source_root)" "$tag" "$commit" \
        "$(release_artifact_root)" "$(release_bundle_root)"
}

verify_release_bundle_for() {
    verify_release_bundle "$(release_bundle_root)" "$1" "$2"
}

require_release_tag_for_package() {
    local tag="${THREETERM_RELEASE_TAG:-}"
    [[ -n "$tag" ]] || fail 'AUR/COPR publication requires THREETERM_RELEASE_TAG'
    valid_tag "$tag" || fail "invalid THREETERM_RELEASE_TAG: ${tag}"
    require_tag_at_head "$tag"
    printf '%s\n' "$tag"
}

release_artifact_root() {
    printf '%s\n' "${THREETERM_RELEASE_ARTIFACT_ROOT:-${ROOT}/target/libslvs-artifact}"
}

release_artifact_manifest() {
    local artifact_root="$1"
    printf '%s\n' "${THREETERM_RELEASE_ARTIFACT_MANIFEST:-${artifact_root}/manifest.json}"
}

current_gate() {
    awk '
        /<!-- CURRENT-GATE:START -->/ { in_gate = 1; next }
        /<!-- CURRENT-GATE:END -->/ { in_gate = 0 }
        in_gate { print }
    ' "$1"
}

row_text() {
    local gate=$1
    local id=$2
    awk -v id="$id" '
        index($0, "**" id "**") && $0 ~ /^- \[[xX]\]/ { in_row = 1 }
        in_row && index($0, "**") && $0 ~ /^- \[[xX]\]/ && index($0, "**" id "**") == 0 { exit }
        in_row { print }
    ' <<<"$gate"
}

require_line() {
    local gate=$1
    local pattern=$2
    if ! grep -Eq "$pattern" <<<"$gate"; then
        fail "current gate is missing required field: ${pattern}"
    fi
}

verify_runbook() {
    local path=$1
    [[ -f "$path" ]] || fail "runbook not found: ${path}"

    local gate
    gate="$(current_gate "$path")"
    [[ -n "$gate" ]] || fail "runbook has no current gate section"

    require_line "$gate" '^Current gate status: `SIGNED`$'
    require_line "$gate" '^Candidate date: `[0-9]{4}-[0-9]{2}-[0-9]{2}`$'
    require_line "$gate" '^Declared product owner: `[^`]+`$'
    require_line "$gate" '^Product-owner authorization date: `[0-9]{4}-[0-9]{2}-[0-9]{2}`$'
    require_line "$gate" '^Release authorization: `APPROVED`$'
    require_line "$gate" '^Evidence freshness window: `30 days`$'

    if grep -Eq '^- \[[^xX]\]' <<<"$gate"; then
        fail "current gate contains an unchecked item"
    fi
    if grep -Eiq 'not (set|recorded|authorized)|unresolved|blocked|record exact|list each .* here|inspect all public-facing uses|example\.(com|invalid)|placeholder|live check|(^|["` ;])(TBD|TODO|unknown|n/a|test result)(["` ;]|$)' <<<"$gate"; then
        fail "current gate contains an incomplete value"
    fi

    local candidate_date candidate_epoch today
    candidate_date="$(grep -E '^Candidate date:' <<<"$gate" | sed -E 's/.*`([^`]*)`.*/\1/')"
    candidate_epoch="$(date -u -d "$candidate_date" +%s 2>/dev/null)" || fail "candidate date is invalid"
    today="$(date -u +%F)"
    [[ "$candidate_date" == "$today" ]] || fail "candidate date ${candidate_date} is not today (${today})"

    local owner auth_date auth_re
    owner="$(grep -E '^Declared product owner:' <<<"$gate" | sed -E 's/.*`([^`]*)`.*/\1/')"
    auth_date="$(grep -E '^Product-owner authorization date:' <<<"$gate" | sed -E 's/.*`([^`]*)`.*/\1/')"
    [[ "$auth_date" == "$candidate_date" ]] || fail "authorization date does not match candidate date"
    auth_re='Product-owner authorization: `([^`]*)`; signed: `([0-9]{4}-[0-9]{2}-[0-9]{2})`'
    if [[ ! "$gate" =~ $auth_re ]]; then
        fail "final product-owner authorization signature is missing"
    fi
    [[ "${BASH_REMATCH[1]}" == "$owner" ]] || fail "final authorization signer does not match declared owner"
    [[ "${BASH_REMATCH[2]}" == "$candidate_date" ]] || fail "final authorization signature date does not match candidate date"

    local id row evidence_date evidence_epoch disposition signoff_re
    for id in "${REQUIRED_ROWS[@]}"; do
        [[ "$(grep -Ec "^- \[[xX]\] \*\*${id}\*\*" <<<"$gate")" == 1 ]] \
            || fail "current gate must contain exactly one checked row for ${id}"
        row="$(row_text "$gate" "$id")"
        require_line "$row" '^  - Evidence date: `[0-9]{4}-[0-9]{2}-[0-9]{2}`$'
        require_line "$row" '^  - Evidence record: `query="[^"]+"; source="(https?://|docs/|\.github/)[^"]+"; result="[^" ]([^" ]| )*"`$'
        require_line "$row" '^  - Sources consulted: `[^`]+`$'
        grep -Fq "${REQUIRED_QUERY_TOKENS[$id]}" <<<"$row" \
            || fail "evidence query for ${id} is missing ${REQUIRED_QUERY_TOKENS[$id]}"
        grep -Fq "${REQUIRED_SOURCE_TOKENS[$id]}" <<<"$row" \
            || fail "evidence source for ${id} is missing ${REQUIRED_SOURCE_TOKENS[$id]}"
        require_line "$row" '^  - Disposition: `\[(PASS|ACCEPTED)\] .+`$'
        signoff_re='Product-owner sign-off: `([^`]*)`; signed: `([0-9]{4}-[0-9]{2}-[0-9]{2})`'
        if [[ ! "$row" =~ $signoff_re ]]; then
            fail "owner sign-off is missing for ${id}"
        fi
        [[ "${BASH_REMATCH[1]}" == "$owner" ]] || fail "owner sign-off for ${id} does not match declared owner"
        [[ "${BASH_REMATCH[2]}" == "$candidate_date" ]] || fail "owner sign-off date for ${id} does not match candidate date"
        evidence_date="$(grep -E '^  - Evidence date:' <<<"$row" | sed -E 's/.*`([^`]*)`.*/\1/')"
        evidence_epoch="$(date -u -d "$evidence_date" +%s 2>/dev/null)" || fail "evidence date is invalid for ${id}"
        (( evidence_epoch <= candidate_epoch )) || fail "evidence for ${id} is in the future"
        (( candidate_epoch - evidence_epoch <= 30 * 86400 )) || fail "evidence for ${id} is older than 30 days"
    done

    local required_text
    required_text=(
        'USPTO' 'WIPO' 'TMview' 'EUIPO' 'relevant national offices'
        'ThreeTerm' '3Term' 'Terminal3' 'Terminal Three' 'Terminal 3'
        '.com' '.app' '.dev' '.io' 'project-specific TLDs'
        'crates.io' 'npm' 'PyPI' 'AUR' 'Homebrew'
        '91298824' 'terminal-native parametric CAD terminology'
        'downloadable or open-source CAD software' 'forbidden in branding'
        'release tag' 'GitHub Release' 'AUR push' 'COPR build'
    )
    for required_text in "${required_text[@]}"; do
        grep -Fq "$required_text" <<<"$gate" \
            || fail "current gate is missing required requirement: ${required_text}"
    done
    printf 'release gate: signed current gate verified\n'
}

verify_checked_in_runbook() {
    verify_runbook "$RUNBOOK"
}

require_committed_runbook() {
    [[ -z "$(git -C "$(release_source_root)" status --porcelain -- "$RUNBOOK")" ]] \
        || fail 'signed runbook must be committed before publishing'
}

require_tag_at_head() {
    local tag=$1 tag_commit head_commit
    tag_commit="$(git -C "$(release_source_root)" rev-parse --verify "${tag}^{commit}" 2>/dev/null)" \
        || fail "release tag does not exist: ${tag}"
    head_commit="$(git -C "$(release_source_root)" rev-parse --verify HEAD)" \
        || fail 'unable to resolve the verified checkout revision'
    [[ "$tag_commit" == "$head_commit" ]] \
        || fail "release tag ${tag} does not point at the verified checkout revision"
}

valid_tag() {
    [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]
}

usage() {
    printf '%s\n' \
        'Usage:' \
        '  release.sh verify [runbook-path]' \
        '  release.sh verify-artifact <manifest> <artifact-root>' \
        '  release.sh build <tag>' \
        '  release.sh tag <annotated-tag>' \
        '  release.sh github-release <tag>' \
        '  release.sh aur-push HEAD:refs/heads/master' \
        '  release.sh copr-build threeterm.spec [--nowait]'
}

action=${1:-}
shift || true
case "$action" in
    verify-artifact)
        [[ $# == 2 ]] || { usage >&2; exit 2; }
        # shellcheck source=/dev/null
        source "${LICENSING_SCRIPT}"
        verify_libslvs_artifact "$1" "$2"
        ;;
    build)
        [[ $# == 1 ]] && valid_tag "$1" || { usage >&2; exit 2; }
        verify_checked_in_runbook
        require_committed_runbook
        commit="$(release_verified_commit)"
        verify_release_material "$commit" "$1"
        build_release_bundle_for "$1" "$commit"
        ;;
    verify)
        [[ $# -le 1 ]] || { usage >&2; exit 2; }
        verify_runbook "${1:-$RUNBOOK}"
        ;;
    tag)
        [[ $# == 1 ]] && valid_tag "$1" || { usage >&2; exit 2; }
        verify_checked_in_runbook
        require_committed_runbook
        commit="$(release_verified_commit)"
        verify_release_material "$commit" "$1"
        build_release_bundle_for "$1" "$commit"
        git -C "$(release_source_root)" tag -a "$1" -m "ThreeTerm $1"
        ;;
    github-release)
        [[ $# == 1 ]] && valid_tag "$1" || { usage >&2; exit 2; }
        verify_checked_in_runbook
        require_committed_runbook
        require_tag_at_head "$1"
        commit="$(release_verified_commit)"
        verify_release_material "$commit" "$1"
        build_release_bundle_for "$1" "$commit"
        command -v gh >/dev/null 2>&1 || fail 'gh is required for GitHub Release'
        bundle_root="$(release_bundle_root)"
        archive="${bundle_root}/threeterm-${1}.tar.gz"
        release_args=("$1" --verify-tag --title "$1")
        if [[ -n "${THREETERM_RELEASE_MATERIAL:-}" ]]; then
            release_args+=(--notes-file "${THREETERM_RELEASE_MATERIAL}")
        fi
        release_args+=(
            "$archive"
            "${bundle_root}/release-manifest.json"
            "${bundle_root}/SHA256SUMS"
            "${bundle_root}/worker-manifest.json"
        )
        if [[ -n "${THREETERM_RELEASE_MATERIAL:-}" ]]; then
            release_args+=("${THREETERM_RELEASE_MATERIAL}")
            release_args+=("${ROOT}/docs/release/six-gate-performance-claims-gate.md")
            if performance_gate_claim_language <"${THREETERM_RELEASE_MATERIAL}"; then
                evidence_path="$(performance_gate_block "${ROOT}/docs/release/six-gate-performance-claims-gate.md" \
                    | grep -E '^evidence_path: ' | cut -d' ' -f2-)"
                release_args+=("${ROOT}/${evidence_path}")
            fi
        fi
        gh release create "${release_args[@]}"
        ;;
    aur-push)
        [[ $# == 1 && "$1" != -* ]] || { usage >&2; exit 2; }
        [[ "$1" == 'HEAD:refs/heads/master' ]] || fail 'AUR push is fixed to HEAD:refs/heads/master'
        verify_checked_in_runbook
        require_committed_runbook
        tag="$(require_release_tag_for_package)"
        commit="$(release_verified_commit)"
        verify_release_material "$commit" "$tag"
        build_release_bundle_for "$tag" "$commit"
        verify_release_bundle_for "$tag" "$commit"
        git -C "$(release_source_root)" push aur HEAD:refs/heads/master
        ;;
    copr-build)
        [[ $# == 1 || ( $# == 2 && "$2" == '--nowait' ) ]] || { usage >&2; exit 2; }
        [[ "$1" == 'threeterm.spec' ]] || fail 'COPR build is fixed to threeterm.spec'
        verify_checked_in_runbook
        require_committed_runbook
        tag="$(require_release_tag_for_package)"
        commit="$(release_verified_commit)"
        verify_release_material "$commit" "$tag"
        build_release_bundle_for "$tag" "$commit"
        verify_release_bundle_for "$tag" "$commit"
        command -v copr >/dev/null 2>&1 || fail 'copr is required for COPR build'
        if [[ $# == 2 ]]; then
            copr build threeterm "$1" --nowait
        else
            copr build threeterm "$1"
        fi
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
