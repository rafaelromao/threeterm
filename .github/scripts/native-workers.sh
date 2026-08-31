#!/usr/bin/env bash
# Immutable native-worker contract for canonical CI. This file is sourced by
# ci.sh so the selected worker paths and digests remain in the test shell.

set -euo pipefail

NATIVE_WORKER_MANIFEST_SCHEMA="threeterm.ci.native-workers/1"
NATIVE_ARCH_IMAGE="docker.io/archlinux@sha256:b860afd5823683f7ea389ba5f00d812f4fe55f6f286dea329d2abeefa535e309"
OCCT_SOURCE_REPOSITORY="https://github.com/Open-Cascade-SAS/OCCT"
OCCT_SOURCE_COMMIT="c5f20409c52bf8f658314d205a0e5d6f0be0969c"
SLVS_SOURCE_REPOSITORY="https://github.com/solvespace/solvespace"
SLVS_SOURCE_COMMIT="27b6a080c8b669421bd4d444650c3b8eddec5687"

native_worker_root() {
    printf '%s\n' "${CARGO_TARGET_DIR:-target}/native-workers"
}

clone_at_commit() {
    local repository="$1"
    local commit="$2"
    local destination="$3"

    if [[ -d "${destination}/.git" ]]; then
        git -C "${destination}" fetch --force --depth 1 origin "${commit}"
    else
        rm -rf "${destination}"
        git clone --no-checkout --filter=blob:none "${repository}" "${destination}"
        git -C "${destination}" fetch --force --depth 1 origin "${commit}"
    fi
    git -C "${destination}" checkout --detach --force "${commit}"
    # SolveSpace keeps required build dependencies in git submodules. Restore
    # them from the checked-out commit rather than relying on mutable packages.
    git -C "${destination}" submodule update --init --recursive
    local actual
    actual="$(git -C "${destination}" rev-parse HEAD)"
    if [[ "${actual}" != "${commit}" ]]; then
        printf 'native worker source mismatch: repository=%s expected=%s actual=%s\n' \
            "${repository}" "${commit}" "${actual}" >&2
        return 1
    fi
}

prepare_native_workers() {
    : "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR must be set before preparing native workers}"
    local root
    root="$(native_worker_root)"
    local sources="${root}/sources"
    local prefixes="${root}/prefixes"
    mkdir -p "${sources}" "${prefixes}"

    if [[ -n "${THREETERM_SKIP_OCCTBUILD:-}" || -n "${THREETERM_SKIP_SLVSBUILD:-}" ]]; then
        printf '%s\n' 'canonical CI forbids native-worker build skips' >&2
        return 1
    fi
    if [[ -n "${THREETERM_OCCTBUILD_WORKER:-}" || -n "${THREETERM_SLVSBUILD_WORKER:-}" ]]; then
        printf '%s\n' 'canonical CI forbids native-worker path overrides' >&2
        return 1
    fi

    clone_at_commit "${OCCT_SOURCE_REPOSITORY}" "${OCCT_SOURCE_COMMIT}" "${sources}/occt"
    cmake -S "${sources}/occt" -B "${root}/occt-build" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="${prefixes}/occt" \
        -DBUILD_TESTING=OFF \
        -DBUILD_MODULE_Draw=OFF \
        -DBUILD_MODULE_Visualization=OFF \
        -DBUILD_MODULE_ApplicationFramework=OFF
    cmake --build "${root}/occt-build" --parallel 2
    cmake --install "${root}/occt-build"

    clone_at_commit "${SLVS_SOURCE_REPOSITORY}" "${SLVS_SOURCE_COMMIT}" "${sources}/slvs"
    cmake -S "${sources}/slvs" -B "${root}/slvs-build" \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="${prefixes}/slvs" \
        -DENABLE_GUI=OFF \
        -DENABLE_CLI=OFF \
        -DENABLE_TESTS=OFF
    cmake --build "${root}/slvs-build" --target slvs --parallel 2
    cmake --install "${root}/slvs-build"

    export THREETERM_OCCT_DIR="${prefixes}/occt"
    export THREETERM_SLVS_DIR="${prefixes}/slvs"
    export THREETERM_OCCT_LIB_DIR="${prefixes}/occt/lib"
    export THREETERM_SLVS_LIB_DIR="${prefixes}/slvs/lib"
    export LD_LIBRARY_PATH="${prefixes}/occt/lib:${prefixes}/slvs/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
    export THREETERM_REQUIRE_IMMUTABLE_WORKERS=1
    export THREETERM_REQUIRE_REAL_WORKER=1
}

selected_worker_path() {
    local name="$1"
    local candidate
    for candidate in "${CARGO_TARGET_DIR}/debug/build/threeterm-${name}-"*/out/bin/threeterm-${name}-worker; do
        if [[ -f "${candidate}" ]]; then
            printf '%s\n' "${candidate}"
            return 0
        fi
    done
    printf 'canonical %s worker was not built under %s\n' "${name}" "${CARGO_TARGET_DIR}" >&2
    return 1
}

verify_native_worker_execution() {
    local worker="$1"
    local worker_id="$2"
    local output
    local status=0
    # Native workers advertise readiness before reading one bounded request.
    # Send a valid envelope with no args so both workers return a structured
    # malformed-request response without mutating a workspace.
    output="$(printf '%s\n' '{"kind":"request","schema_version":"threeterm.protocol/1","request_id":"ci-probe","command_id":"ci_probe"}' | timeout 10s "${worker}" 2>&1)" || status=$?
    if [[ "${status}" -ne 2 || "${output}" != *"\"kind\":\"worker_ready\""* || "${output}" != *"\"worker_id\":\"${worker_id}\""* || "${output}" != *"\"code\":\"request_malformed\""* ]]; then
        printf 'native %s worker did not complete its ready handshake: %s (status=%s)\n' \
            "${worker_id}" "${worker}" "${status}" >&2
        printf '%s\n' "${output}" >&2
        return 1
    fi
}

worker_linked_libraries() {
    local binary="$1"
    ldd "${binary}" | while read -r _ arrow path _; do
        if [[ "${arrow:-}" == "=>" && "${path:-}" == /* && -f "${path}" ]]; then
            jq -cn --arg path "${path}" --arg sha256 "$(sha256sum "${path}" | cut -d' ' -f1)" \
                '{path: $path, sha256: $sha256}'
        fi
    done | jq -s 'unique_by(.path) | sort_by(.path)'
}

finalize_native_worker_manifest() {
    local occt_worker="$1"
    local slvs_worker="$2"
    local executed="${3:-false}"
    local manifest="${CARGO_TARGET_DIR}/native-worker-manifest.json"
    local temporary="${manifest}.tmp.$$"
    local occt_sha256
    local slvs_sha256
    occt_sha256="$(sha256sum "${occt_worker}" | cut -d' ' -f1)"
    slvs_sha256="$(sha256sum "${slvs_worker}" | cut -d' ' -f1)"

    jq -n \
        --arg schema_version "${NATIVE_WORKER_MANIFEST_SCHEMA}" \
        --arg container_image "${NATIVE_ARCH_IMAGE}" \
        --arg occt_repository "${OCCT_SOURCE_REPOSITORY}" \
        --arg occt_commit "${OCCT_SOURCE_COMMIT}" \
        --arg occt_path "${occt_worker}" \
        --arg occt_sha256 "${occt_sha256}" \
        --argjson occt_executed "${executed}" \
        --argjson occt_libraries "$(worker_linked_libraries "${occt_worker}")" \
        --arg slvs_repository "${SLVS_SOURCE_REPOSITORY}" \
        --arg slvs_commit "${SLVS_SOURCE_COMMIT}" \
        --arg slvs_path "${slvs_worker}" \
        --arg slvs_sha256 "${slvs_sha256}" \
        --argjson slvs_executed "${executed}" \
        --argjson slvs_libraries "$(worker_linked_libraries "${slvs_worker}")" \
        '{schema_version: $schema_version,
          container_image: $container_image,
          workers: {
            occt: {source_repository: $occt_repository, source_commit: $occt_commit,
                   package_identity: "source-commit",
                    executed: $occt_executed,
                   executable: {path: $occt_path, sha256: $occt_sha256}, linked_libraries: $occt_libraries},
            libslvs: {source_repository: $slvs_repository, source_commit: $slvs_commit,
                      package_identity: "source-commit",
                      executed: $slvs_executed,
                      executable: {path: $slvs_path, sha256: $slvs_sha256}, linked_libraries: $slvs_libraries}
          }}' > "${temporary}"
    mv "${temporary}" "${manifest}"

    jq -e '.schema_version == "threeterm.ci.native-workers/1" and
           (.workers.occt.executable.sha256 | length == 64) and
           (.workers.libslvs.executable.sha256 | length == 64)' "${manifest}" >/dev/null
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    printf '%s\n' 'native-workers.sh must be sourced by ci.sh' >&2
    exit 2
fi
