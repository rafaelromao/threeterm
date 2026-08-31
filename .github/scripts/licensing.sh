#!/usr/bin/env bash
# Shared licensing contract for source trees and relocatable worker artifacts.

set -euo pipefail

LIBSLVS_POLICY_RELATIVE="licenses/libslvs.json"

licensing_fail() {
    printf 'licensing:%s: %s\n' "$1" "$2" >&2
    return 1
}

licensing_require_file() {
    local root="$1"
    local relative="$2"
    [[ -n "$relative" && "$relative" != /* && "$relative" != *..* ]] \
        || licensing_fail path "unsafe licensing path: ${relative}"
    [[ -f "${root}/${relative}" ]] \
        || licensing_fail missing-file "required licensing file is missing: ${relative}"
}

licensing_sha256() {
    sha256sum "$1" | cut -d' ' -f1
}

verify_libslvs_source() {
    local root="$1"
    local policy="${root}/${LIBSLVS_POLICY_RELATIVE}"
    [[ -f "$policy" ]] || licensing_fail missing-policy "policy is missing: ${LIBSLVS_POLICY_RELATIVE}"
    command -v jq >/dev/null 2>&1 || licensing_fail tool "jq is required"

    local basis worker_id worker_schema source_repository source_commit source_url
    local license_text notice source_offer
    basis="$(jq -er '.basis' "$policy")" || licensing_fail policy "policy basis is missing"
    worker_id="$(jq -er '.worker_id' "$policy")" || licensing_fail policy "policy worker identity is missing"
    worker_schema="$(jq -er '.worker_schema_version' "$policy")" || licensing_fail policy "policy worker schema is missing"
    source_repository="$(jq -er '.source_repository' "$policy")" || licensing_fail policy "policy source repository is missing"
    source_commit="$(jq -er '.source_commit' "$policy")" || licensing_fail policy "policy source commit is missing"
    source_url="$(jq -er '.source_url' "$policy")" || licensing_fail policy "policy immutable source URL is missing"
    license_text="$(jq -er '.license_text' "$policy")" || licensing_fail policy "policy license text path is missing"
    notice="$(jq -er '.notice' "$policy")" || licensing_fail policy "policy NOTICE path is missing"
    source_offer="$(jq -er '.source_offer' "$policy")" || licensing_fail policy "policy source-offer path is missing"

    [[ "$basis" == GPL-3.0-only ]] || licensing_fail basis "unsupported libslvs distribution basis: ${basis}"
    [[ "$worker_id" == slvs ]] || licensing_fail worker-identity "unexpected worker identity: ${worker_id}"
    [[ "$worker_schema" == threeterm.workers.slvs/1 ]] || licensing_fail worker-schema "unexpected worker schema: ${worker_schema}"
    [[ "$source_repository" == https://github.com/solvespace/solvespace ]] \
        || licensing_fail source-repository "unexpected SolveSpace source repository"
    [[ "$source_commit" =~ ^[0-9a-f]{40}$ ]] || licensing_fail source-commit "source commit is not a full SHA"
    [[ "$source_url" == "${source_repository}/tree/${source_commit}" ]] \
        || licensing_fail source-url "source URL must identify the pinned commit"

    licensing_require_file "$root" "$license_text"
    licensing_require_file "$root" "$notice"
    licensing_require_file "$root" "$source_offer"
    [[ "$(wc -l < "${root}/${license_text}")" -ge 300 ]] \
        || licensing_fail license-text "GPLv3 license text is incomplete"
    grep -Fq 'GNU GENERAL PUBLIC LICENSE' "${root}/${license_text}" \
        || licensing_fail license-text "GPLv3 heading is missing"
    grep -Fq 'END OF TERMS AND CONDITIONS' "${root}/${license_text}" \
        || licensing_fail license-text "GPLv3 terms are incomplete"

    grep -Fq "$basis" "${root}/${notice}" \
        || licensing_fail notice "NOTICE does not name ${basis}"
    grep -Fq "$source_commit" "${root}/${notice}" \
        || licensing_fail notice "NOTICE does not name the pinned source commit"
    grep -Fq "$source_url" "${root}/${notice}" \
        || licensing_fail notice "NOTICE does not name the immutable source URL"
    grep -Fq 'SOURCE-OFFER.txt' "${root}/${notice}" \
        || licensing_fail notice "NOTICE does not identify the source offer"

    grep -Fq 'valid for at least three years' "${root}/${source_offer}" \
        || licensing_fail source-offer "source offer has no three-year validity"
    grep -Fq 'Corresponding Source' "${root}/${source_offer}" \
        || licensing_fail source-offer "source offer does not promise Corresponding Source"
    grep -Fq 'at no charge' "${root}/${source_offer}" \
        || licensing_fail source-offer "source offer has no no-charge delivery term"
    grep -Fq "$source_url" "${root}/${source_offer}" \
        || licensing_fail source-offer "source offer does not identify the pinned source"

    local metadata package_license metadata_basis metadata_worker metadata_commit
    metadata="$(cargo metadata --no-deps --format-version 1 --manifest-path "${root}/Cargo.toml")" \
        || licensing_fail cargo-metadata "cargo metadata could not inspect the workspace"
    package_license="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .license' <<<"$metadata")" \
        || licensing_fail cargo-package "threeterm-slvs-worker is missing from Cargo metadata"
    [[ "$package_license" == "$basis" ]] \
        || licensing_fail cargo-license "Cargo license ${package_license} disagrees with ${basis}"
    metadata_basis="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.basis' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo licensing metadata is missing"
    metadata_worker="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.worker_id' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo worker identity metadata is missing"
    metadata_commit="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.source_commit' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo source commit metadata is missing"
    [[ "$metadata_basis" == "$basis" && "$metadata_worker" == "$worker_id" && "$metadata_commit" == "$source_commit" ]] \
        || licensing_fail cargo-metadata "Cargo licensing metadata disagrees with policy"

    grep -Fq "constexpr const char* kWorkerSchema = \"${worker_schema}\"" \
        "${root}/crates/workers/slvs/src-cpp/worker_main.cpp" \
        || licensing_fail worker-identity "worker schema does not agree with policy"
    grep -Fq 'worker_id\":\"slvs' "${root}/crates/workers/slvs/src-cpp/worker_main.cpp" \
        || licensing_fail worker-identity "worker readiness identity does not agree with policy"
    grep -Fq "const SLVS_SHA: &str = \"${source_commit}\"" "${root}/crates/workers/slvs/build.rs" \
        || licensing_fail worker-source "worker build source commit does not agree with policy"
    grep -Fq "pub const SOURCE_COMMIT: &str = \"${source_commit}\"" \
        "${root}/crates/workers/slvs/src/lib.rs" \
        || licensing_fail worker-source "worker runtime source commit does not agree with policy"

    printf '%s\n' 'libslvs licensing source verified'
}

verify_libslvs_artifact() {
    local manifest="$1"
    local root="${2:-$(dirname "$manifest")}"
    [[ -f "$manifest" ]] || licensing_fail missing-manifest "artifact manifest is missing: ${manifest}"
    command -v jq >/dev/null 2>&1 || licensing_fail tool "jq is required"

    [[ "$(jq -er '.schema_version' "$manifest")" == threeterm.release.libslvs/1 ]] \
        || licensing_fail manifest-schema "unsupported libslvs artifact manifest schema"
    local name worker_id worker_schema basis source_commit source_url
    name="$(jq -er '.artifact.name' "$manifest")" || licensing_fail manifest "artifact name is missing"
    worker_id="$(jq -er '.artifact.worker_id' "$manifest")" || licensing_fail worker-identity "artifact worker identity is missing"
    worker_schema="$(jq -er '.artifact.worker_schema_version' "$manifest")" || licensing_fail worker-schema "artifact worker schema is missing"
    basis="$(jq -er '.licensing.basis' "$manifest")" || licensing_fail basis "artifact license basis is missing"
    source_commit="$(jq -er '.licensing.source_commit' "$manifest")" || licensing_fail source-commit "artifact source commit is missing"
    source_url="$(jq -er '.licensing.source_url' "$manifest")" || licensing_fail source-url "artifact source URL is missing"
    [[ "$name" == threeterm-slvs-worker && "$worker_id" == slvs && "$worker_schema" == threeterm.workers.slvs/1 ]] \
        || licensing_fail worker-identity "artifact worker identity is inconsistent"
    [[ "$basis" == GPL-3.0-only && "$source_commit" == 27b6a080c8b669421bd4d444650c3b8eddec5687 ]] \
        || licensing_fail basis "artifact licensing basis is inconsistent"
    [[ "$source_url" == "https://github.com/solvespace/solvespace/tree/${source_commit}" ]] \
        || licensing_fail source-url "artifact source URL is not immutable"

    local count path digest actual
    count="$(jq '.files | length' "$manifest")"
    [[ "$count" == 5 ]] || licensing_fail manifest "artifact manifest must list five required files"
    while IFS=$'\t' read -r path digest; do
        licensing_require_file "$root" "$path"
        actual="$(licensing_sha256 "${root}/${path}")"
        [[ "$actual" == "$digest" ]] || licensing_fail digest "digest mismatch for ${path}"
    done < <(jq -r '.files[] | [.path, .sha256] | @tsv' "$manifest")

    local text notice offer
    text="$(jq -er '.licensing.license_text.path' "$manifest")"
    notice="$(jq -er '.licensing.notice.path' "$manifest")"
    offer="$(jq -er '.licensing.source_offer.path' "$manifest")"
    grep -Fq 'GNU GENERAL PUBLIC LICENSE' "${root}/${text}" \
        || licensing_fail license-text "artifact GPLv3 text is incomplete"
    grep -Fq 'SOURCE-OFFER.txt' "${root}/${notice}" \
        || licensing_fail notice "artifact NOTICE is incomplete"
    grep -Fq 'valid for at least three years' "${root}/${offer}" \
        || licensing_fail source-offer "artifact source offer is incomplete"
    printf '%s\n' 'libslvs licensing artifact verified'
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    case "${1:-}" in
        verify-source) verify_libslvs_source "${2:?repository root required}" ;;
        verify-artifact) verify_libslvs_artifact "${2:?manifest required}" "${3:-}" ;;
        *) printf '%s\n' 'Usage: licensing.sh verify-source <repo-root> | verify-artifact <manifest> <artifact-root>' >&2; exit 2 ;;
    esac
fi
