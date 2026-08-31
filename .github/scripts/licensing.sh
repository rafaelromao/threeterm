#!/usr/bin/env bash
# Shared licensing contract for source trees and relocatable worker artifacts.

set -euo pipefail

LIBSLVS_POLICY_RELATIVE="licenses/libslvs.json"
LICENSING_FAILED=0

licensing_fail() {
    printf 'licensing:%s: %s\n' "$1" "$2" >&2
    LICENSING_FAILED=1
    return 1
}

licensing_require_file() {
    local root="$1"
    local relative="$2"
    [[ -n "$relative" && "$relative" != /* && "$relative" != *..* ]] \
        || licensing_fail path "unsafe licensing path: ${relative}"
    [[ -f "${root}/${relative}" && ! -L "${root}/${relative}" ]] \
        || licensing_fail missing-file "required licensing file is missing: ${relative}"
}

licensing_sha256() {
    sha256sum "$1" | cut -d' ' -f1
}

verify_libslvs_source() {
    LICENSING_FAILED=0
    local root="$1"
    local policy="${root}/${LIBSLVS_POLICY_RELATIVE}"
    [[ -f "$policy" ]] || licensing_fail missing-policy "policy is missing: ${LIBSLVS_POLICY_RELATIVE}"
    command -v jq >/dev/null 2>&1 || licensing_fail tool "jq is required"

    local basis worker_id worker_schema source_repository source_commit source_url
    local license_text license_text_sha256 notice source_offer
    basis="$(jq -er '.basis' "$policy")" || licensing_fail policy "policy basis is missing"
    worker_id="$(jq -er '.worker_id' "$policy")" || licensing_fail policy "policy worker identity is missing"
    worker_schema="$(jq -er '.worker_schema_version' "$policy")" || licensing_fail policy "policy worker schema is missing"
    source_repository="$(jq -er '.source_repository' "$policy")" || licensing_fail policy "policy source repository is missing"
    source_commit="$(jq -er '.source_commit' "$policy")" || licensing_fail policy "policy source commit is missing"
    source_url="$(jq -er '.source_url' "$policy")" || licensing_fail policy "policy immutable source URL is missing"
    license_text="$(jq -er '.license_text' "$policy")" || licensing_fail policy "policy license text path is missing"
    license_text_sha256="$(jq -er '.license_text_sha256' "$policy")" \
        || licensing_fail policy "policy license text digest is missing"
    notice="$(jq -er '.notice' "$policy")" || licensing_fail policy "policy NOTICE path is missing"
    source_offer="$(jq -er '.source_offer' "$policy")" || licensing_fail policy "policy source-offer path is missing"

    [[ "$basis" == GPL-3.0-only ]] || licensing_fail basis "unsupported libslvs distribution basis: ${basis}"
    [[ "$worker_id" == slvs ]] || licensing_fail worker-identity "unexpected worker identity: ${worker_id}"
    [[ "$worker_schema" == threeterm.workers.slvs/1 ]] || licensing_fail worker-schema "unexpected worker schema: ${worker_schema}"
    [[ "$source_repository" == https://github.com/solvespace/solvespace ]] \
        || licensing_fail source-repository "unexpected SolveSpace source repository"
    [[ "$source_commit" == 27b6a080c8b669421bd4d444650c3b8eddec5687 ]] \
        || licensing_fail source-commit "source commit is not the approved SolveSpace revision"
    [[ "$source_url" == "${source_repository}/tree/${source_commit}" ]] \
        || licensing_fail source-url "source URL must identify the pinned commit"

    licensing_require_file "$root" "$license_text"
    licensing_require_file "$root" "$notice"
    licensing_require_file "$root" "$source_offer"
    [[ "$(licensing_sha256 "${root}/${license_text}")" == "$license_text_sha256" ]] \
        || licensing_fail license-text "GPLv3 license text digest does not match the approved text"
    [[ "$(wc -l < "${root}/${license_text}")" -ge 300 ]] \
        || licensing_fail license-text "GPLv3 license text is incomplete"
    grep -Fq 'GNU GENERAL PUBLIC LICENSE' "${root}/${license_text}" \
        || { licensing_fail license-text "GPLv3 heading is missing"; return 1; }
    grep -Fq 'END OF TERMS AND CONDITIONS' "${root}/${license_text}" \
        || { licensing_fail license-text "GPLv3 terms are incomplete"; return 1; }

    grep -Fq "$basis" "${root}/${notice}" \
        || { licensing_fail notice "NOTICE does not name ${basis}"; return 1; }
    grep -Fq "$source_commit" "${root}/${notice}" \
        || { licensing_fail notice "NOTICE does not name the pinned source commit"; return 1; }
    grep -Fq "$source_url" "${root}/${notice}" \
        || { licensing_fail notice "NOTICE does not name the immutable source URL"; return 1; }
    grep -Fq 'SOURCE-OFFER.txt' "${root}/${notice}" \
        || { licensing_fail notice "NOTICE does not identify the source offer"; return 1; }
    grep -Fq 'LICENSES/GPL-3.0-only.txt' "${root}/${notice}" \
        || { licensing_fail notice "NOTICE does not identify the staged license path"; return 1; }

    grep -Fq 'valid for at least three years' "${root}/${source_offer}" \
        || { licensing_fail source-offer "source offer has no three-year validity"; return 1; }
    grep -Fq 'Corresponding Source' "${root}/${source_offer}" \
        || { licensing_fail source-offer "source offer does not promise Corresponding Source"; return 1; }
    grep -Fq 'at no charge' "${root}/${source_offer}" \
        || { licensing_fail source-offer "source offer has no no-charge delivery term"; return 1; }
    grep -Fq "$source_url" "${root}/${source_offer}" \
        || { licensing_fail source-offer "source offer does not identify the pinned source"; return 1; }
    grep -Fq 'https://github.com/rafaelromao/threeterm' "${root}/${source_offer}" \
        || { licensing_fail source-offer "source offer does not identify the ThreeTerm source"; return 1; }
    grep -Fq 'source/crates/workers/slvs' "${root}/${source_offer}" \
        || { licensing_fail source-offer "source offer does not identify the bundled worker source"; return 1; }
    grep -Fq 'https://github.com/rafaelromao/threeterm' "${root}/${source_offer}" \
        || licensing_fail source-offer "source offer does not identify the ThreeTerm source"

    local metadata package_license metadata_basis metadata_worker metadata_commit metadata_policy
    local metadata_url metadata_license_text metadata_notice metadata_offer
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
    metadata_policy="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.policy' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo licensing policy metadata is missing"
    metadata_url="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.source_url' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo source URL metadata is missing"
    metadata_license_text="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.license_text' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo license text metadata is missing"
    metadata_notice="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.notice' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo NOTICE metadata is missing"
    metadata_offer="$(jq -er '.packages[] | select(.name == "threeterm-slvs-worker") | .metadata.threeterm.licensing.source_offer' <<<"$metadata")" \
        || licensing_fail cargo-metadata "Cargo source-offer metadata is missing"
    [[ "$metadata_basis" == "$basis" && "$metadata_worker" == "$worker_id" && "$metadata_commit" == "$source_commit" \
        && "$metadata_policy" == ../../../licenses/libslvs.json \
        && "$metadata_url" == "$source_url" && "$metadata_license_text" == LICENSE-GPL-3.0.txt \
        && "$metadata_notice" == NOTICE && "$metadata_offer" == SOURCE-OFFER.txt ]] \
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

    if (( LICENSING_FAILED )); then return 1; fi
    printf '%s\n' 'libslvs licensing source verified'
}

stage_libslvs_artifact() {
    local source_root="$1"
    local worker="$2"
    local artifact_root="$3"
    verify_libslvs_source "$source_root"
    [[ -f "$worker" && -x "$worker" ]] \
        || licensing_fail worker "selected libslvs worker is not executable: ${worker}"

    rm -rf "$artifact_root"
    mkdir -p "${artifact_root}/bin" "${artifact_root}/licenses" "${artifact_root}/LICENSES"
    cp "$worker" "${artifact_root}/bin/threeterm-slvs-worker"
    cp "${source_root}/licenses/libslvs.json" "${artifact_root}/licenses/libslvs.json"
    cp "${source_root}/crates/workers/slvs/LICENSE-GPL-3.0.txt" \
        "${artifact_root}/LICENSES/GPL-3.0-only.txt"
    cp "${source_root}/crates/workers/slvs/NOTICE" "${artifact_root}/NOTICE"
    cp "${source_root}/crates/workers/slvs/SOURCE-OFFER.txt" "${artifact_root}/SOURCE-OFFER.txt"
    mkdir -p "${artifact_root}/source/crates/workers/slvs/src" \
        "${artifact_root}/source/crates/workers/slvs/src-cpp"
    cp "${source_root}/crates/workers/slvs/Cargo.toml" \
        "${artifact_root}/source/crates/workers/slvs/Cargo.toml"
    cp "${source_root}/crates/workers/slvs/build.rs" \
        "${artifact_root}/source/crates/workers/slvs/build.rs"
    cp "${source_root}/crates/workers/slvs/src/lib.rs" \
        "${artifact_root}/source/crates/workers/slvs/src/lib.rs"
    cp "${source_root}/crates/workers/slvs/src/envelope.rs" \
        "${artifact_root}/source/crates/workers/slvs/src/envelope.rs"
    cp "${source_root}/crates/workers/slvs/src-cpp/worker_main.cpp" \
        "${artifact_root}/source/crates/workers/slvs/src-cpp/worker_main.cpp"

    local manifest="${artifact_root}/manifest.json"
    local temporary="${manifest}.tmp.$$"
    local file_paths=(
        bin/threeterm-slvs-worker
        licenses/libslvs.json
        LICENSES/GPL-3.0-only.txt
        NOTICE
        SOURCE-OFFER.txt
        source/crates/workers/slvs/Cargo.toml
        source/crates/workers/slvs/build.rs
        source/crates/workers/slvs/src/lib.rs
        source/crates/workers/slvs/src/envelope.rs
        source/crates/workers/slvs/src-cpp/worker_main.cpp
    )
    local files='[]' path digest
    for path in "${file_paths[@]}"; do
        digest="$(licensing_sha256 "${artifact_root}/${path}")"
        files="$(jq -c --arg path "$path" --arg sha256 "$digest" \
            '. + [{path: $path, sha256: $sha256}]' <<<"$files")"
    done
    jq -n \
        --arg schema_version threeterm.release.libslvs/1 \
        --arg name threeterm-slvs-worker \
        --arg worker_id slvs \
        --arg worker_schema_version threeterm.workers.slvs/1 \
        --arg basis GPL-3.0-only \
        --arg source_repository https://github.com/solvespace/solvespace \
        --arg source_commit 27b6a080c8b669421bd4d444650c3b8eddec5687 \
        --arg source_url https://github.com/solvespace/solvespace/tree/27b6a080c8b669421bd4d444650c3b8eddec5687 \
        --arg executable_path bin/threeterm-slvs-worker \
        --arg executable_sha256 "$(licensing_sha256 "${artifact_root}/bin/threeterm-slvs-worker")" \
        --arg policy_path licenses/libslvs.json \
        --arg policy_sha256 "$(licensing_sha256 "${artifact_root}/licenses/libslvs.json")" \
        --arg license_text_path LICENSES/GPL-3.0-only.txt \
        --arg license_text_sha256 "$(licensing_sha256 "${artifact_root}/LICENSES/GPL-3.0-only.txt")" \
        --arg notice_path NOTICE \
        --arg notice_sha256 "$(licensing_sha256 "${artifact_root}/NOTICE")" \
        --arg source_offer_path SOURCE-OFFER.txt \
        --arg source_offer_sha256 "$(licensing_sha256 "${artifact_root}/SOURCE-OFFER.txt")" \
        --argjson files "$files" \
        '{schema_version: $schema_version,
          artifact: {name: $name, worker_id: $worker_id,
                     worker_schema_version: $worker_schema_version,
                     executable: {path: $executable_path, sha256: $executable_sha256}},
          licensing: {basis: $basis, source_repository: $source_repository,
                      source_commit: $source_commit, source_url: $source_url,
                      policy: {path: $policy_path, sha256: $policy_sha256},
                      license_text: {path: $license_text_path, sha256: $license_text_sha256},
                      notice: {path: $notice_path, sha256: $notice_sha256},
                      source_offer: {path: $source_offer_path, sha256: $source_offer_sha256}},
          files: $files}' >"$temporary"
    mv "$temporary" "$manifest"
    verify_libslvs_artifact "$manifest" "$artifact_root"
}

verify_libslvs_artifact() {
    LICENSING_FAILED=0
    local manifest="$1"
    local root="${2:-$(dirname "$manifest")}"
    [[ -f "$manifest" ]] || licensing_fail missing-manifest "artifact manifest is missing: ${manifest}"
    command -v jq >/dev/null 2>&1 || licensing_fail tool "jq is required"

    [[ "$(jq -er '.schema_version' "$manifest")" == threeterm.release.libslvs/1 ]] \
        || licensing_fail manifest-schema "unsupported libslvs artifact manifest schema"
    local name worker_id worker_schema basis source_repository source_commit source_url
    name="$(jq -er '.artifact.name' "$manifest")" || licensing_fail manifest "artifact name is missing"
    worker_id="$(jq -er '.artifact.worker_id' "$manifest")" || licensing_fail worker-identity "artifact worker identity is missing"
    worker_schema="$(jq -er '.artifact.worker_schema_version' "$manifest")" || licensing_fail worker-schema "artifact worker schema is missing"
    basis="$(jq -er '.licensing.basis' "$manifest")" || licensing_fail basis "artifact license basis is missing"
    source_repository="$(jq -er '.licensing.source_repository' "$manifest")" \
        || licensing_fail source-repository "artifact source repository is missing"
    source_commit="$(jq -er '.licensing.source_commit' "$manifest")" || licensing_fail source-commit "artifact source commit is missing"
    source_url="$(jq -er '.licensing.source_url' "$manifest")" || licensing_fail source-url "artifact source URL is missing"
    [[ "$name" == threeterm-slvs-worker && "$worker_id" == slvs && "$worker_schema" == threeterm.workers.slvs/1 ]] \
        || licensing_fail worker-identity "artifact worker identity is inconsistent"
    [[ "$basis" == GPL-3.0-only && "$source_repository" == https://github.com/solvespace/solvespace \
        && "$source_commit" == 27b6a080c8b669421bd4d444650c3b8eddec5687 ]] \
        || licensing_fail basis "artifact licensing basis is inconsistent"
    [[ "$source_url" == "https://github.com/solvespace/solvespace/tree/${source_commit}" ]] \
        || licensing_fail source-url "artifact source URL is not immutable"

    local executable_path executable_sha256
    executable_path="$(jq -er '.artifact.executable.path' "$manifest")" \
        || licensing_fail worker "artifact executable path is missing"
    executable_sha256="$(jq -er '.artifact.executable.sha256' "$manifest")" \
        || licensing_fail worker "artifact executable digest is missing"
    licensing_require_file "$root" "$executable_path"
    [[ -x "${root}/${executable_path}" ]] || licensing_fail worker "artifact worker is not executable"
    [[ "$(licensing_sha256 "${root}/${executable_path}")" == "$executable_sha256" ]] \
        || licensing_fail digest "artifact executable digest is inconsistent"

    local count path digest actual
    count="$(jq '.files | length' "$manifest")"
    [[ "$count" == 10 ]] || licensing_fail manifest "artifact manifest must list ten required files"
    jq -e '.files | map(.path) == [
        "bin/threeterm-slvs-worker",
        "licenses/libslvs.json",
        "LICENSES/GPL-3.0-only.txt",
        "NOTICE",
        "SOURCE-OFFER.txt",
        "source/crates/workers/slvs/Cargo.toml",
        "source/crates/workers/slvs/build.rs",
        "source/crates/workers/slvs/src/lib.rs",
        "source/crates/workers/slvs/src/envelope.rs",
        "source/crates/workers/slvs/src-cpp/worker_main.cpp"
    ]' "$manifest" >/dev/null \
        || licensing_fail manifest "artifact manifest file list is incomplete or inconsistent"
    while IFS=$'\t' read -r path digest; do
        if ! licensing_require_file "$root" "$path"; then
            return 1
        fi
        actual="$(licensing_sha256 "${root}/${path}")"
        if [[ "$actual" != "$digest" ]]; then
            licensing_fail digest "digest mismatch for ${path}"
            return 1
        fi
    done <<<"$(jq -r '.files[] | [.path, .sha256] | @tsv' "$manifest")"

    local policy policy_digest text text_digest notice notice_digest offer offer_digest
    policy="$(jq -er '.licensing.policy.path' "$manifest")" \
        || licensing_fail policy "artifact policy path is missing"
    policy_digest="$(jq -er '.licensing.policy.sha256' "$manifest")" \
        || licensing_fail policy "artifact policy digest is missing"
    if ! licensing_require_file "$root" "$policy"; then return 1; fi
    [[ "$(licensing_sha256 "${root}/${policy}")" == "$policy_digest" ]] \
        || { licensing_fail digest "artifact policy digest is inconsistent"; return 1; }
    [[ "$(jq -er '.basis' "${root}/${policy}")" == "$basis" \
        && "$(jq -er '.worker_id' "${root}/${policy}")" == "$worker_id" \
        && "$(jq -er '.worker_schema_version' "${root}/${policy}")" == "$worker_schema" \
        && "$(jq -er '.source_repository' "${root}/${policy}")" == "$source_repository" \
        && "$(jq -er '.source_commit' "${root}/${policy}")" == "$source_commit" \
        && "$(jq -er '.source_url' "${root}/${policy}")" == "$source_url" ]] \
        || { licensing_fail basis "artifact policy disagrees with manifest"; return 1; }
    text="$(jq -er '.licensing.license_text.path' "$manifest")"
    text_digest="$(jq -er '.licensing.license_text.sha256' "$manifest")" \
        || licensing_fail license-text "artifact license text digest is missing"
    notice="$(jq -er '.licensing.notice.path' "$manifest")"
    notice_digest="$(jq -er '.licensing.notice.sha256' "$manifest")" \
        || licensing_fail notice "artifact NOTICE digest is missing"
    offer="$(jq -er '.licensing.source_offer.path' "$manifest")"
    offer_digest="$(jq -er '.licensing.source_offer.sha256' "$manifest")" \
        || licensing_fail source-offer "artifact source offer digest is missing"
    for entry in "$text:$text_digest" "$notice:$notice_digest" "$offer:$offer_digest"; do
        path="${entry%%:*}"
        digest="${entry#*:}"
        if ! licensing_require_file "$root" "$path"; then return 1; fi
        [[ "$(licensing_sha256 "${root}/${path}")" == "$digest" ]] \
            || { licensing_fail digest "artifact digest is inconsistent for ${path}"; return 1; }
    done
    [[ "$text" == LICENSES/GPL-3.0-only.txt && "$notice" == NOTICE && "$offer" == SOURCE-OFFER.txt ]] \
        || { licensing_fail manifest "artifact licensing paths are inconsistent"; return 1; }
    [[ "$(jq -er '.license_text_sha256' "${root}/${policy}")" == "$text_digest" ]] \
        || { licensing_fail license-text "artifact license text is not the approved GPLv3 text"; return 1; }
    grep -Fq 'GNU GENERAL PUBLIC LICENSE' "${root}/${text}" \
        || { licensing_fail license-text "artifact GPLv3 text is incomplete"; return 1; }
    [[ "$(wc -l < "${root}/${text}")" -ge 300 ]] \
        || { licensing_fail license-text "artifact GPLv3 text is incomplete"; return 1; }
    grep -Fq 'END OF TERMS AND CONDITIONS' "${root}/${text}" \
        || { licensing_fail license-text "artifact GPLv3 terms are incomplete"; return 1; }
    grep -Fq "$basis" "${root}/${notice}" \
        || { licensing_fail notice "artifact NOTICE does not name the license basis"; return 1; }
    grep -Fq "$source_commit" "${root}/${notice}" \
        || { licensing_fail notice "artifact NOTICE does not name the pinned source"; return 1; }
    grep -Fq "$source_url" "${root}/${notice}" \
        || { licensing_fail notice "artifact NOTICE does not name the immutable source URL"; return 1; }
    grep -Fq 'SOURCE-OFFER.txt' "${root}/${notice}" \
        || { licensing_fail notice "artifact NOTICE is incomplete"; return 1; }
    grep -Fq 'valid for at least three years' "${root}/${offer}" \
        || { licensing_fail source-offer "artifact source offer is incomplete"; return 1; }
    grep -Fq 'Corresponding Source' "${root}/${offer}" \
        || { licensing_fail source-offer "artifact source offer omits Corresponding Source"; return 1; }
    grep -Fq 'at no charge' "${root}/${offer}" \
        || { licensing_fail source-offer "artifact source offer omits no-charge delivery"; return 1; }
    grep -Fq "$source_url" "${root}/${offer}" \
        || { licensing_fail source-offer "artifact source offer omits the pinned source"; return 1; }
    grep -Fq 'https://github.com/rafaelromao/threeterm' "${root}/${offer}" \
        || { licensing_fail source-offer "artifact source offer omits the ThreeTerm source"; return 1; }
    grep -Fq 'source/crates/workers/slvs' "${root}/${offer}" \
        || { licensing_fail source-offer "artifact source offer omits the bundled source"; return 1; }
    if (( LICENSING_FAILED )); then return 1; fi
    printf '%s\n' 'libslvs licensing artifact verified'
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    case "${1:-}" in
        verify-source) verify_libslvs_source "${2:?repository root required}" ;;
        verify-artifact) verify_libslvs_artifact "${2:?manifest required}" "${3:-}" ;;
        *) printf '%s\n' 'Usage: licensing.sh verify-source <repo-root> | verify-artifact <manifest> <artifact-root>' >&2; exit 2 ;;
    esac
fi
