#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}" || exit 1
RECIPE="${THREETERM_THREE_JOURNEY_RECIPE:-${ROOT}/crates/host/tests/data/bracket_complete_recipe.v1.json}"
RUN_ROOT="${THREETERM_THREE_JOURNEY_ROOT:-${ROOT}/target/three-journey-gate}"
RUN_ID="${THREETERM_JOURNEY_RUN_ID:-}"
TIMEOUT_SECONDS="${THREETERM_THREE_JOURNEY_TIMEOUT_SECONDS:-900}"
KILL_GRACE_SECONDS="${THREETERM_THREE_JOURNEY_KILL_GRACE_SECONDS:-10}"
MODE="all"
REQUESTED_SURFACE=""
SYNTHESIZED_STATUS=""
SYNTHESIZED_REASON=""

usage() {
    cat <<'EOF'
Usage:
  three-journey-gate.sh                 Run API, MCP, TUI, then aggregate.
  three-journey-gate.sh --surface NAME  Run one producer (api|mcp|tui).
  three-journey-gate.sh --synthesize NAME STATUS REASON
                                         Record a job that never produced evidence.
  three-journey-gate.sh --aggregate      Aggregate retained producer evidence only.

Environment:
  THREETERM_THREE_JOURNEY_ROOT             Run-scoped evidence root.
  THREETERM_JOURNEY_RUN_ID                 Stable run ID shared by all jobs.
  THREETERM_THREE_JOURNEY_TIMEOUT_SECONDS  Per-journey timeout (default 900).
  THREETERM_THREE_JOURNEY_KILL_GRACE_SECONDS
                                            Process-group kill grace (default 10).

The TUI producer requires a qualified direct Ghostty runner with the
threeterm-graphical label. Missing prerequisites are recorded as skipped and
cannot pass the aggregate.
EOF
}

while (($# > 0)); do
    case "$1" in
        --surface)
            [[ $# -ge 2 ]] || { usage >&2; exit 2; }
            MODE="surface"
            REQUESTED_SURFACE="$2"
            shift 2
            ;;
        --aggregate)
            MODE="aggregate"
            shift
            ;;
        --synthesize)
            [[ $# -ge 4 ]] || { usage >&2; exit 2; }
            MODE="synthesize"
            REQUESTED_SURFACE="$2"
            SYNTHESIZED_STATUS="$3"
            SYNTHESIZED_REASON="$4"
            shift 4
            ;;
        all)
            MODE="all"
            shift
            ;;
        --help)
            usage
            exit 0
            ;;
        *)
            usage >&2
            exit 2
            ;;
    esac
done

case "${MODE}" in
    surface)
        case "${REQUESTED_SURFACE}" in
            api|mcp|tui) ;;
            *) printf 'unknown producer surface: %s\n' "${REQUESTED_SURFACE}" >&2; exit 2 ;;
        esac
        ;;
    synthesize)
        case "${REQUESTED_SURFACE}" in
            api|mcp|tui) ;;
            *) printf 'unknown synthesized surface: %s\n' "${REQUESTED_SURFACE}" >&2; exit 2 ;;
        esac
        case "${SYNTHESIZED_STATUS}" in
            unrun|failed|prerequisite_skipped) ;;
            *) printf 'unknown synthesized status: %s\n' "${SYNTHESIZED_STATUS}" >&2; exit 2 ;;
        esac
        ;;
    aggregate|all) ;;
esac

if [[ ! "${TIMEOUT_SECONDS}" =~ ^[1-9][0-9]*$ || ! "${KILL_GRACE_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s\n' 'journey gate timeout values must be positive integers' >&2
    exit 2
fi

EVIDENCE_ROOT="${RUN_ROOT}/evidence"
COVERAGE_ROOT="${RUN_ROOT}/coverage"
ATTEMPTS_ROOT="${RUN_ROOT}/attempts"
PROCESS_ROOT="${RUN_ROOT}/process"
MANIFEST="${RUN_ROOT}/run.json"
CATALOG="${THREETERM_THREE_JOURNEY_CATALOG:-${RUN_ROOT}/catalog.json}"
PINNED_OCCT_WORKER=""
PINNED_SLVS_WORKER=""
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${RUN_ROOT}/target}"

mkdir -p "${RUN_ROOT}" "${EVIDENCE_ROOT}" "${COVERAGE_ROOT}" "${ATTEMPTS_ROOT}" "${PROCESS_ROOT}"

if [[ -z "${RUN_ID}" ]]; then
    RUN_ID="$(git -C "${ROOT}" rev-parse HEAD)-$(date +%s%N)"
fi
if [[ ! "${RUN_ID}" =~ ^[A-Za-z0-9._-]+$ || "${RUN_ID}" == . || "${RUN_ID}" == .. ]]; then
    printf '%s\n' 'run ID must be a safe path component' >&2
    exit 2
fi

if [[ "${MODE}" == all || "${THREETERM_THREE_JOURNEY_RESET:-false}" == true ]]; then
    if ! rm -rf -- "${EVIDENCE_ROOT}" "${COVERAGE_ROOT}" "${ATTEMPTS_ROOT}" \
        "${PROCESS_ROOT}" "${RUN_ROOT}/worker-bundle" "${MANIFEST}" "${CATALOG}"; then
        printf '%s\n' 'unable to reset the run evidence root' >&2
        exit 1
    fi
    if ! mkdir -p "${EVIDENCE_ROOT}" "${COVERAGE_ROOT}" "${ATTEMPTS_ROOT}" "${PROCESS_ROOT}"; then
        printf '%s\n' 'unable to recreate the run evidence root' >&2
        exit 1
    fi
fi

canonical_recipe_sha256() {
    cargo run -q -p threeterm-host --bin threeterm-journey-gate -- --recipe-digest "${RECIPE}"
}

write_manifest() {
    if [[ -f "${MANIFEST}" ]]; then
        [[ "$(jq -er '.run_id' "${MANIFEST}" 2>/dev/null)" == "${RUN_ID}" ]] || {
            printf '%s\n' 'existing run manifest belongs to a different run ID' >&2
            return 1
        }
        return 0
    fi
    if [[ "${MODE}" == aggregate || "${MODE}" == synthesize ]]; then
        printf '%s\n' 'aggregate input is missing its producer-created run manifest' >&2
        return 1
    fi
    local commit dirty canonical raw temporary
    commit="$(git -C "${ROOT}" rev-parse HEAD 2>/dev/null || true)"
    [[ "${commit}" =~ ^[0-9a-f]{40}$ ]] || commit='unknown'
    if [[ -n "$(git -C "${ROOT}" status --porcelain --untracked-files=all 2>/dev/null)" ]]; then
        dirty=true
    else
        dirty=false
    fi
    canonical="$(canonical_recipe_sha256)" || return 1
    raw="$(sha256sum "${RECIPE}" | cut -d' ' -f1)" || return 1
    temporary="${MANIFEST}.tmp.$$"
    jq -n \
        --arg schema_version 'threeterm.acceptance.run/1' \
        --arg run_id "${RUN_ID}" \
        --arg commit "${commit}" \
        --argjson dirty "${dirty}" \
        --arg recipe_schema 'threeterm.recipe.bracket-complete/1' \
        --arg canonical "${canonical}" \
        --arg raw "${raw}" \
        '{schema_version: $schema_version, run_id: $run_id,
          source: {commit: $commit, dirty: $dirty},
          recipe: {schema_version: $recipe_schema,
                   canonical_json_sha256: $canonical, file_sha256: $raw},
          journeys: [
            {surface: "api", test: "e2e_stl_api_all_tools_l_bracket"},
            {surface: "mcp", test: "e2e_stl_mcp_all_tools_l_bracket"},
            {surface: "tui", test: "production_tui_all_tools_stl_journey"}
          ]}' >"${temporary}" || return 1
    mv -f -- "${temporary}" "${MANIFEST}"
    RUN_ID="$(jq -er '.run_id' "${MANIFEST}")"
}

write_coverage_binding() {
    local surface="$1"
    local report="${COVERAGE_ROOT}/${surface}-journey-coverage.json"
    local temporary="${COVERAGE_ROOT}/${surface}-journey-coverage.binding.json.tmp.$$"
    [[ -f "${report}" ]] || {
        printf 'coverage report is missing for %s\n' "${surface}" >&2
        return 1
    }
    jq -n \
        --arg schema_version 'threeterm.acceptance.coverage-binding/1' \
        --arg run_id "${RUN_ID}" --arg surface "${surface}" \
        --arg path "$(basename "${report}")" \
        --argjson bytes "$(wc -c <"${report}" | tr -d ' ')" \
        --arg sha256 "$(sha256sum "${report}" | cut -d' ' -f1)" \
        '{schema_version: $schema_version, run_id: $run_id, surface: $surface,
          report: {path: $path, bytes: $bytes, sha256: $sha256}}' \
        >"${temporary}" && mv -f -- "${temporary}" \
            "${COVERAGE_ROOT}/${surface}-journey-coverage.binding.json"
}

validate_worker_bundle_manifest() {
    local manifest="$1" occt_worker="$2" slvs_worker="$3"
    local occt_sha256 slvs_sha256
    [[ -f "${manifest}" && -x "${occt_worker}" && -x "${slvs_worker}" ]] || return 1
    occt_sha256="$(sha256sum "${occt_worker}" | cut -d' ' -f1)"
    slvs_sha256="$(sha256sum "${slvs_worker}" | cut -d' ' -f1)"
    jq -e \
        --arg image "${NATIVE_ARCH_IMAGE}" \
        --arg occt_repository "${OCCT_SOURCE_REPOSITORY}" \
        --arg occt_commit "${OCCT_SOURCE_COMMIT}" \
        --arg slvs_repository "${SLVS_SOURCE_REPOSITORY}" \
        --arg slvs_commit "${SLVS_SOURCE_COMMIT}" \
        --arg occt_sha256 "${occt_sha256}" --arg slvs_sha256 "${slvs_sha256}" '
        .schema_version == "threeterm.ci.native-workers/2" and
        .container_image == $image and
        .workers.occt.worker_id == "occt" and
        .workers.libslvs.worker_id == "slvs" and
        .workers.occt.worker_schema_version == "threeterm.workers.occt/1" and
        .workers.libslvs.worker_schema_version == "threeterm.workers.slvs/1" and
        .workers.occt.protocol_schema_version == "threeterm.protocol/1" and
        .workers.libslvs.protocol_schema_version == "threeterm.protocol/1" and
        .workers.occt.source_repository == $occt_repository and
        .workers.occt.source_commit == $occt_commit and
        .workers.libslvs.source_repository == $slvs_repository and
        .workers.libslvs.source_commit == $slvs_commit and
        .workers.occt.package_identity == "source-commit" and
        .workers.libslvs.package_identity == "source-commit" and
        .workers.occt.executed == true and
        .workers.libslvs.executed == true and
        .workers.occt.executable.sha256 == $occt_sha256 and
        .workers.libslvs.executable.sha256 == $slvs_sha256 and
        (.workers.occt.executable.path | type == "string" and length > 0) and
        (.workers.libslvs.executable.path | type == "string" and length > 0) and
        (.workers.occt.executable.sha256 | test("^[0-9a-f]{64}$")) and
        (.workers.libslvs.executable.sha256 | test("^[0-9a-f]{64}$")) and
        (.workers.occt.linked_libraries | type == "array" and length > 0) and
        (.workers.libslvs.linked_libraries | type == "array" and length > 0) and
        all(.workers.occt.linked_libraries[]; (.path | type == "string") and (.sha256 | test("^[0-9a-f]{64}$"))) and
        all(.workers.libslvs.linked_libraries[]; (.path | type == "string") and (.sha256 | test("^[0-9a-f]{64}$"))) and
        .libslvs_artifact.manifest_path == "libslvs-artifact/manifest.json" and
        (.libslvs_artifact.manifest_sha256 | test("^[0-9a-f]{64}$"))
    ' "${manifest}" >/dev/null
    local bundle_root
    bundle_root="$(dirname "${manifest}")"
    verify_libslvs_artifact "${bundle_root}/libslvs-artifact/manifest.json" \
        "${bundle_root}/libslvs-artifact" || return 1
    [[ "$(sha256sum "${bundle_root}/libslvs-artifact/manifest.json" | cut -d' ' -f1)" \
        == "$(jq -er '.libslvs_artifact.manifest_sha256' "${manifest}")" ]] || return 1
}

verify_bundle_linked_libraries() {
    local manifest="$1"
    local path expected candidate found
    for worker in occt libslvs; do
        while IFS=$'\t' read -r path expected; do
            found=false
            for candidate in \
                "${path}" \
                "${THREETERM_OCCT_LIB_DIR}/$(basename "${path}")" \
                "${THREETERM_SLVS_LIB_DIR}/$(basename "${path}")"; do
                if [[ -f "${candidate}" ]] && [[ "$(sha256sum "${candidate}" | cut -d' ' -f1)" == "${expected}" ]]; then
                    found=true
                    break
                fi
            done
            [[ "${found}" == true ]] || return 1
        done < <(jq -r ".workers.${worker}.linked_libraries[] | [.path, .sha256] | @tsv" "${manifest}")
    done
}

relative_path() {
    realpath --relative-to="${RUN_ROOT}" "$1"
}

write_attempt() {
    local surface="$1" status="$2" prerequisite_status="$3" prerequisite_reason="$4"
    local command="$5" exit_status="$6" timed_out="$7" duration_ms="$8" failure="$9"
    local stdout_path="${PROCESS_ROOT}/${surface}.stdout"
    local stderr_path="${PROCESS_ROOT}/${surface}.stderr"
    local stdout_bytes=0 stderr_bytes=0 stdout_sha='' stderr_sha=''
    [[ -f "${stdout_path}" ]] && stdout_bytes="$(wc -c <"${stdout_path}" | tr -d ' ')" && stdout_sha="$(sha256sum "${stdout_path}" | cut -d' ' -f1)"
    [[ -f "${stderr_path}" ]] && stderr_bytes="$(wc -c <"${stderr_path}" | tr -d ' ')" && stderr_sha="$(sha256sum "${stderr_path}" | cut -d' ' -f1)"
    local test
    case "${surface}" in
        api) test='e2e_stl_api_all_tools_l_bracket' ;;
        mcp) test='e2e_stl_mcp_all_tools_l_bracket' ;;
        tui) test='production_tui_all_tools_stl_journey' ;;
    esac
    local temporary="${ATTEMPTS_ROOT}/${surface}.json.tmp.$$"
    jq -n \
        --arg schema_version 'threeterm.acceptance.attempt/1' \
        --arg run_id "${RUN_ID}" --arg surface "${surface}" --arg test "${test}" \
        --arg status "${status}" --arg command "${command}" \
        --argjson exit_status "${exit_status}" --argjson timed_out "${timed_out}" \
        --argjson duration_ms "${duration_ms}" \
        --arg prerequisite_status "${prerequisite_status}" \
        --arg prerequisite_reason "${prerequisite_reason}" \
        --arg stdout "$(relative_path "${stdout_path}")" --argjson stdout_bytes "${stdout_bytes}" --arg stdout_sha "${stdout_sha}" \
        --arg stderr "$(relative_path "${stderr_path}")" --argjson stderr_bytes "${stderr_bytes}" --arg stderr_sha "${stderr_sha}" \
        --arg failure "${failure}" \
        '{schema_version: $schema_version, run_id: $run_id, surface: $surface,
          test: $test, status: $status, command: $command,
          exit_status: $exit_status, timed_out: $timed_out,
          duration_ms: $duration_ms,
          prerequisite: {status: $prerequisite_status, reason: $prerequisite_reason},
          stdout: {path: $stdout, bytes: $stdout_bytes, sha256: $stdout_sha},
          stderr: {path: $stderr, bytes: $stderr_bytes, sha256: $stderr_sha},
          failure: (if $failure == "" then null else $failure end)}' \
        >"${temporary}" && mv -f -- "${temporary}" "${ATTEMPTS_ROOT}/${surface}.json"
}

prepare_native_inputs() {
    if [[ "${THREETERM_THREE_JOURNEY_SKIP_NATIVE:-false}" == true ]]; then
        return 0
    fi
    if [[ -n "${THREETERM_OCCTBUILD_WORKER:-}" && -x "${THREETERM_OCCTBUILD_WORKER}" \
        && -n "${THREETERM_SLVSBUILD_WORKER:-}" && -x "${THREETERM_SLVSBUILD_WORKER}" ]]; then
        [[ -n "${THREETERM_OCCT_DIR:-}" && -d "${THREETERM_OCCT_DIR}" ]] || return 1
        [[ -n "${THREETERM_SLVS_DIR:-}" && -d "${THREETERM_SLVS_DIR}" ]] || return 1
        local bundled_occt="${THREETERM_OCCTBUILD_WORKER}"
        local bundled_slvs="${THREETERM_SLVSBUILD_WORKER}"
        local built_occt built_slvs
        # Build metadata normally so immutable mode remains enforced, then
        # replace the generated executables with the verified handoff bundle.
        unset THREETERM_OCCTBUILD_WORKER THREETERM_SLVSBUILD_WORKER
        unset THREETERM_SKIP_OCCTBUILD THREETERM_SKIP_SLVSBUILD
        export THREETERM_REQUIRE_IMMUTABLE_WORKERS=1
        source "${ROOT}/.github/scripts/native-workers.sh" || return 2
        export THREETERM_OCCT_LIB_DIR="${THREETERM_OCCT_DIR}/lib"
        export THREETERM_SLVS_LIB_DIR="${THREETERM_SLVS_DIR}/lib"
        validate_worker_bundle_manifest \
            "${THREETERM_NATIVE_WORKER_MANIFEST:-}" "${bundled_occt}" "${bundled_slvs}" || return 2
        verify_bundle_linked_libraries "${THREETERM_NATIVE_WORKER_MANIFEST}" || return 2
        export LD_LIBRARY_PATH="${THREETERM_OCCT_DIR}/lib:${THREETERM_SLVS_DIR}/lib${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
        cargo build -p threeterm-occt-worker -p threeterm-slvs-worker --jobs 1 || return 2
        built_occt="$(selected_worker_path occt)" || return 2
        built_slvs="$(selected_worker_path slvs)" || return 2
        cp -- "${bundled_occt}" "${built_occt}" || return 2
        cp -- "${bundled_slvs}" "${built_slvs}" || return 2
        PINNED_OCCT_WORKER="${built_occt}"
        PINNED_SLVS_WORKER="${built_slvs}"
        export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${PINNED_OCCT_WORKER}" | cut -d' ' -f1)"
        export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${PINNED_SLVS_WORKER}" | cut -d' ' -f1)"
        verify_native_worker_execution "${PINNED_OCCT_WORKER}" occt || return 2
        verify_native_worker_execution "${PINNED_SLVS_WORKER}" slvs || return 2
        return 0
    fi
    # shellcheck source=/dev/null
    source "${ROOT}/.github/scripts/native-workers.sh" || return 2
    local cached_occt cached_slvs
    if cached_occt="$(selected_worker_path occt 2>/dev/null)" \
        && cached_slvs="$(selected_worker_path slvs 2>/dev/null)" \
        && [[ -f "$(native_worker_root)/native-worker-manifest.json" ]]; then
        export THREETERM_OCCT_DIR="$(native_worker_root)/prefixes/occt"
        export THREETERM_SLVS_DIR="$(native_worker_root)/prefixes/slvs"
        export THREETERM_OCCT_LIB_DIR="${THREETERM_OCCT_DIR}/lib"
        export THREETERM_SLVS_LIB_DIR="${THREETERM_SLVS_DIR}/lib"
        export LD_LIBRARY_PATH="${THREETERM_OCCT_LIB_DIR}:${THREETERM_SLVS_LIB_DIR}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
        PINNED_OCCT_WORKER="${cached_occt}"
        PINNED_SLVS_WORKER="${cached_slvs}"
        export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${PINNED_OCCT_WORKER}" | cut -d' ' -f1)"
        export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${PINNED_SLVS_WORKER}" | cut -d' ' -f1)"
        verify_native_worker_manifest || return 2
        [[ "$(jq -er '.workers.occt.executable.path' "$(native_worker_root)/native-worker-manifest.json")" == "${cached_occt}" ]] || return 2
        [[ "$(jq -er '.workers.libslvs.executable.path' "$(native_worker_root)/native-worker-manifest.json")" == "${cached_slvs}" ]] || return 2
        verify_native_worker_execution "${PINNED_OCCT_WORKER}" occt || return 2
        verify_native_worker_execution "${PINNED_SLVS_WORKER}" slvs || return 2
        return 0
    fi
    prepare_native_workers || return 2
    cargo build --workspace --jobs 1 || return 2
    PINNED_OCCT_WORKER="$(selected_worker_path occt)" || return 2
    PINNED_SLVS_WORKER="$(selected_worker_path slvs)" || return 2
    export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${PINNED_OCCT_WORKER}" | cut -d' ' -f1)"
    export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${PINNED_SLVS_WORKER}" | cut -d' ' -f1)"
    verify_native_worker_execution "${PINNED_OCCT_WORKER}" occt || return 2
    verify_native_worker_execution "${PINNED_SLVS_WORKER}" slvs || return 2
    finalize_native_worker_manifest "${PINNED_OCCT_WORKER}" "${PINNED_SLVS_WORKER}" true || return 2
    verify_native_worker_manifest || return 2
}

check_prerequisites() {
    local surface="$1"
    if [[ "${surface}" == tui ]]; then
        [[ -n "${THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT:-}" && -f "${THREETERM_GRAPHICAL_TOOLCHAIN_CONTRACT}" ]] || {
            printf '%s\n' 'graphical toolchain contract is missing' >&2
            return 1
        }
    fi
    [[ -n "${PINNED_OCCT_WORKER}" && -x "${PINNED_OCCT_WORKER}" ]] || {
        printf '%s\n' 'pinned OCCT worker is missing' >&2
        return 1
    }
    [[ -n "${PINNED_SLVS_WORKER}" && -x "${PINNED_SLVS_WORKER}" ]] || {
        printf '%s\n' 'pinned libslvs worker is missing' >&2
        return 1
    }
}

surface_command() {
    case "$1" in
        api) printf '%s\n' cargo test -p threeterm-host --test bracket_base_qualification e2e_stl_api_all_tools_l_bracket --jobs 1 -- --include-ignored --exact --test-threads=1 ;;
        mcp) printf '%s\n' cargo test -p threeterm-mcp --test production_lifecycle e2e_stl_mcp_all_tools_l_bracket --jobs 1 -- --include-ignored --exact --test-threads=1 ;;
        tui) printf '%s\n' cargo test -p threeterm-tui --test graphical_launch production_tui_all_tools_stl_journey --jobs 1 -- --include-ignored --exact --test-threads=1 ;;
    esac
}

run_surface() {
    local surface="$1" command_text pid watchdog_pid status started_ms finished_ms duration_ms
    local prepare_status check_status cleanup_status=0
    local timed_out=false prerequisite_status=passed prerequisite_reason='qualified'
    local failure=''
    local stdout_path="${PROCESS_ROOT}/${surface}.stdout" stderr_path="${PROCESS_ROOT}/${surface}.stderr"
    local -a command
    mapfile -t command < <(surface_command "${surface}")
    command_text="${command[*]}"
    if ! rm -rf -- "${EVIDENCE_ROOT}/${RUN_ID}/${surface}"; then
        cleanup_status=1
    fi
    if ! rm -f -- "${COVERAGE_ROOT}/${surface}-journey-coverage.json" \
        "${COVERAGE_ROOT}/${surface}-journey-coverage.binding.json" \
        "${ATTEMPTS_ROOT}/${surface}.json"; then
        cleanup_status=1
    fi
    : >"${stdout_path}"
    : >"${stderr_path}"
    if [[ "${cleanup_status}" -ne 0 ]]; then
        failure='unable to clear stale producer evidence'
        printf '%s\n' "${failure}" >>"${stderr_path}"
        if ! write_attempt "${surface}" failed failed "${failure}" "${command_text}" 1 false 0 "${failure}"; then
            return 1
        fi
        return 1
    fi
    if ! write_attempt "${surface}" unrun unknown 'producer has not started' "${command_text}" null false 0 'attempt created before producer launch'; then
        return 1
    fi
    prepare_status=0
    prepare_native_inputs >>"${stdout_path}" 2>>"${stderr_path}" || prepare_status=$?
    if [[ "${prepare_status}" -eq 2 ]]; then
        failure='native worker preparation or integrity validation failed'
        if ! write_attempt "${surface}" failed failed "${failure}" "${command_text}" 2 false 0 "${failure}"; then
            return 1
        fi
        return 1
    fi
    if [[ "${prepare_status}" -ne 0 ]]; then
        prerequisite_status=skipped
        prerequisite_reason='required native or graphical prerequisite is unavailable'
        printf '%s\n' "${prerequisite_reason}" >>"${stderr_path}"
        if ! write_attempt "${surface}" prerequisite_skipped "${prerequisite_status}" "${prerequisite_reason}" "${command_text}" null false 0 "${prerequisite_reason}"; then
            return 1
        fi
        return 1
    fi
    check_status=0
    check_prerequisites "${surface}" 2>>"${stderr_path}" || check_status=$?
    if [[ "${check_status}" -ne 0 ]]; then
        prerequisite_status=skipped
        prerequisite_reason='required native or graphical prerequisite is unavailable'
        printf '%s\n' "${prerequisite_reason}" >>"${stderr_path}"
        if ! write_attempt "${surface}" prerequisite_skipped "${prerequisite_status}" "${prerequisite_reason}" "${command_text}" null false 0 "${prerequisite_reason}"; then
            return 1
        fi
        return 1
    fi
    export THREETERM_JOURNEY_RUN_ID="${RUN_ID}"
    export THREETERM_JOURNEY_EVIDENCE_ROOT="${EVIDENCE_ROOT}"
    export THREETERM_COVERAGE_EVIDENCE_ROOT="${COVERAGE_ROOT}"
    export THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 THREETERM_REQUIRE_IMMUTABLE_WORKERS=1
    started_ms="$(date +%s%3N)"
    setsid --wait -- "${command[@]}" >"${stdout_path}" 2>"${stderr_path}" &
    pid=$!
    setsid --wait -- bash -e -u -o pipefail -c '
        sleep "$1"
        if kill -0 "$2" 2>/dev/null; then
            printf "%s\n" "journey gate timeout; terminating process group" >>"$3"
            printf "%s\n" timed_out >"$4"
            kill -TERM -- "-$2" 2>/dev/null || kill -TERM "$2" 2>/dev/null || true
            sleep "$5"
            if kill -0 -- "-$2" 2>/dev/null; then
                printf "%s\n" "journey gate kill grace expired" >>"$3"
                kill -KILL -- "-$2" 2>/dev/null || kill -KILL "$2" 2>/dev/null || true
            fi
        fi
    ' _ "${TIMEOUT_SECONDS}" "${pid}" "${stderr_path}" "${stderr_path}.timeout" "${KILL_GRACE_SECONDS}" &
    watchdog_pid=$!
    if wait "${pid}"; then status=0; else status=$?; fi
    if [[ -f "${stderr_path}.timeout" ]]; then
        wait "${watchdog_pid}" 2>/dev/null || true
    else
        kill -TERM -- "-${watchdog_pid}" 2>/dev/null || kill -TERM "${watchdog_pid}" 2>/dev/null || true
        wait "${watchdog_pid}" 2>/dev/null || true
    fi
    finished_ms="$(date +%s%3N)"
    duration_ms=$((finished_ms - started_ms))
    if [[ -f "${stderr_path}.timeout" ]]; then
        timed_out=true
        status=124
        rm -f -- "${stderr_path}.timeout"
    fi
    if [[ "${status}" -eq 0 ]]; then
        if ! write_coverage_binding "${surface}"; then
            status=1
            failure='producer passed but coverage evidence was not retained'
        else
            if ! write_attempt "${surface}" passed passed qualified "${command_text}" 0 "${timed_out}" "${duration_ms}" ''; then
                return 1
            fi
            return 0
        fi
    fi
    [[ -n "${failure}" ]] || failure="producer exited with status ${status}"
    if [[ "${surface}" == tui && -f "${EVIDENCE_ROOT}/manifest.json" ]] && jq -e '.result == "failed" and (.failure.code == "prerequisite_missing" or .failure.code == "toolchain_contract_missing" or .failure.code == "compositor_unavailable" or .failure.code == "capability_or_readiness_failed")' "${EVIDENCE_ROOT}/manifest.json" >/dev/null 2>&1; then
        prerequisite_status=skipped
        prerequisite_reason="$(jq -r '.failure.code' "${EVIDENCE_ROOT}/manifest.json")"
        if ! write_attempt "${surface}" prerequisite_skipped "${prerequisite_status}" "${prerequisite_reason}" "${command_text}" "${status}" "${timed_out}" "${duration_ms}" "${failure}"; then
            return 1
        fi
    else
        if ! write_attempt "${surface}" failed passed qualified "${command_text}" "${status}" "${timed_out}" "${duration_ms}" "${failure}"; then
            return 1
        fi
    fi
    return 1
}

aggregate() {
    cargo run -p threeterm-host --bin threeterm-journey-gate -- \
        --manifest "${MANIFEST}" --attempts "${ATTEMPTS_ROOT}" \
        --coverage "${COVERAGE_ROOT}" --evidence "${EVIDENCE_ROOT}" \
        --recipe "${RECIPE}" --run-id "${RUN_ID}" --catalog "${CATALOG}"
}

manifest_status=0
write_manifest || manifest_status=$?
if [[ "${MODE}" != aggregate && "${MODE}" != synthesize && "${manifest_status}" -ne 0 ]]; then
    exit "${manifest_status}"
fi
overall=0
case "${MODE}" in
    surface)
        run_surface "${REQUESTED_SURFACE}" || overall=1
        ;;
    all)
        for surface in api mcp tui; do
            run_surface "${surface}" || overall=1
        done
        aggregate || overall=1
        ;;
    aggregate)
        aggregate || overall=1
        ;;
    synthesize)
        command_text="$(surface_command "${REQUESTED_SURFACE}")"
        : >"${PROCESS_ROOT}/${REQUESTED_SURFACE}.stdout"
        printf '%s\n' "${SYNTHESIZED_REASON}" >"${PROCESS_ROOT}/${REQUESTED_SURFACE}.stderr"
        prerequisite_status=unknown
        [[ "${SYNTHESIZED_STATUS}" == prerequisite_skipped ]] && prerequisite_status=skipped
        if ! write_attempt "${REQUESTED_SURFACE}" "${SYNTHESIZED_STATUS}" "${prerequisite_status}" \
            "${SYNTHESIZED_REASON}" "${command_text}" null false 0 "${SYNTHESIZED_REASON}"; then
            overall=1
        fi
        ;;
esac
if [[ "${MODE}" == aggregate || "${MODE}" == synthesize ]]; then
    overall=$((overall || manifest_status))
fi
exit "${overall}"
