#!/usr/bin/env bash
# Commit-bound ThreeTerm production conformance catalog.
#
# Every gate runs in its own fail-fast subprocess. The parent keeps running so
# a failed gate never hides later evidence, then publishes the failure catalog
# before returning a non-zero status.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)" || {
    printf '%s\n' 'acceptance catalog: unable to resolve repository root' >&2
    exit 1
}
cd "${ROOT}" || {
    printf '%s\n' 'acceptance catalog: unable to enter repository root' >&2
    exit 1
}

export CARGO_TARGET_DIR="${ROOT}/target/acceptance-run"
CATALOG="${THREETERM_ACCEPTANCE_CATALOG:-${ROOT}/target/acceptance-catalog.json}"
LOG_ROOT="${CARGO_TARGET_DIR}/logs"
NATIVE_MANIFEST="${CARGO_TARGET_DIR}/native-worker-manifest.json"
LIBSLVS_ARTIFACT="${CARGO_TARGET_DIR}/libslvs-artifact"
ARTIFACT_MANIFEST_RELATIVE='libslvs-artifact/manifest.json'
SCHEMA_PROJECT="${CARGO_TARGET_DIR}/schema-project"
SCHEMA_RESPONSE="${CARGO_TARGET_DIR}/schema-response.json"
OCCT_SMOKE_EVIDENCE="${CARGO_TARGET_DIR}/occt-geometry-smoke/real-occt-geometry-smoke.json"
EXPECTED_OCCT_SOURCE_REPOSITORY='https://github.com/Open-Cascade-SAS/OCCT'
EXPECTED_OCCT_SOURCE_COMMIT='c5f20409c52bf8f658314d205a0e5d6f0be0969c'
EXPECTED_OCCT_WORKER_SCHEMA='threeterm.workers.occt/1'
EXPECTED_PROTOCOL_SCHEMA='threeterm.protocol/1'
GATE_TIMEOUT_SECONDS="${THREETERM_ACCEPTANCE_GATE_TIMEOUT_SECONDS:-900}"
GATE_KILL_GRACE_SECONDS="${THREETERM_ACCEPTANCE_GATE_KILL_GRACE_SECONDS:-10}"
if [[ ! "${GATE_TIMEOUT_SECONDS}" =~ ^[1-9][0-9]*$ || ! "${GATE_KILL_GRACE_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s\n' 'acceptance catalog: gate timeout and kill grace must be positive integers' >&2
    exit 1
fi
if ! mkdir -p "$(dirname "${CATALOG}")"; then
    printf '%s\n' 'acceptance catalog: unable to create catalog directory' >&2
    exit 1
fi
if ! rm -rf -- "${CARGO_TARGET_DIR}"; then
    printf '%s\n' 'acceptance catalog: unable to clear acceptance run directory' >&2
    exit 1
fi
if ! mkdir -p "${LOG_ROOT}"; then
    printf '%s\n' 'acceptance catalog: unable to create acceptance log directory' >&2
    exit 1
fi

readonly CATALOG LOG_ROOT NATIVE_MANIFEST LIBSLVS_ARTIFACT ARTIFACT_MANIFEST_RELATIVE SCHEMA_PROJECT SCHEMA_RESPONSE OCCT_SMOKE_EVIDENCE EXPECTED_OCCT_SOURCE_REPOSITORY EXPECTED_OCCT_SOURCE_COMMIT EXPECTED_OCCT_WORKER_SCHEMA EXPECTED_PROTOCOL_SCHEMA GATE_TIMEOUT_SECONDS GATE_KILL_GRACE_SECONDS
export ROOT SOURCE_COMMIT SOURCE_CLEAN LIBSLVS_ARTIFACT SCHEMA_PROJECT SCHEMA_RESPONSE

SOURCE_COMMIT="$(git rev-parse HEAD 2>/dev/null || printf '%s' unknown)"
SOURCE_CLEAN=true
SOURCE_STATUS=''
if ! SOURCE_STATUS="$(git status --porcelain --untracked-files=all 2>/dev/null)"; then
    SOURCE_CLEAN=false
elif [[ -n "${SOURCE_STATUS}" ]]; then
    SOURCE_CLEAN=false
fi

declare -a GATE_IDS=()
declare -a GATE_COMMANDS=()
declare -a GATE_STATUSES=()
declare -a GATE_EXITS=()
declare -a GATE_LOGS=()
declare -a GATE_LOG_BYTES=()
declare -a GATE_LOG_SHA256=()
declare -a GATE_TIMED_OUT=()
declare -a GATE_DURATIONS_MS=()
FAILURE_COUNT=0

run_gate() {
    local id="$1"
    local display="$2"
    shift 2
    local log="${LOG_ROOT}/${id//[^A-Za-z0-9_.-]/_}.log"
    local timeout_marker="${log}.timeout"
    local status
    local started_ms finished_ms duration_ms
    local pid watchdog_pid timed_out=false
    local command_text

    rm -f -- "${timeout_marker}"
    printf -v command_text '%q ' "$@"
    command_text="${command_text% }"
    {
        printf 'description: %s\n' "${display}"
        printf 'command: %s\n' "${command_text}"
    } >"${log}"
    started_ms="$(date +%s%3N)"
    setsid --wait -- "$@" >>"${log}" 2>&1 &
    pid=$!
    setsid --wait -- bash -e -u -o pipefail -c "
        sleep \"\$1\"
        if kill -0 \"\$2\" 2>/dev/null; then
            printf '%s\\n' 'gate watchdog: timeout expired; terminating process group' >>\"\$3\"
            printf '%s\\n' timed_out >\"\$4\"
            kill -TERM -- \"-\$2\" 2>/dev/null || kill -TERM \"\$2\" 2>/dev/null || true
            sleep \"\$5\"
            if kill -0 -- \"-\$2\" 2>/dev/null; then
                printf '%s\\n' 'gate watchdog: grace period expired; killing process group' >>\"\$3\"
                kill -KILL -- \"-\$2\" 2>/dev/null || kill -KILL \"\$2\" 2>/dev/null || true
            fi
        fi
    " _ "${GATE_TIMEOUT_SECONDS}" "${pid}" "${log}" "${timeout_marker}" "${GATE_KILL_GRACE_SECONDS}" &
    watchdog_pid=$!
    if wait "${pid}"; then
        status=0
    else
        status=$?
    fi
    kill -TERM -- "-${watchdog_pid}" 2>/dev/null || kill -TERM "${watchdog_pid}" 2>/dev/null || true
    if kill -0 -- "-${watchdog_pid}" 2>/dev/null; then
        kill -KILL -- "-${watchdog_pid}" 2>/dev/null || true
    fi
    wait "${watchdog_pid}" 2>/dev/null || true
    finished_ms="$(date +%s%3N)"
    duration_ms=$((finished_ms - started_ms))
    if [[ -f "${timeout_marker}" ]]; then
        timed_out=true
        status=124
        rm -f -- "${timeout_marker}"
    fi

    GATE_IDS+=("${id}")
    GATE_COMMANDS+=("${command_text}")
    GATE_EXITS+=("${status}")
    GATE_LOGS+=("${log}")
    GATE_LOG_BYTES+=("$(wc -c <"${log}" | tr -d ' ')")
    GATE_LOG_SHA256+=("$(sha256sum "${log}" | cut -d' ' -f1)")
    GATE_TIMED_OUT+=("${timed_out}")
    GATE_DURATIONS_MS+=("${duration_ms}")
    if [[ "${status}" -eq 0 ]]; then
        GATE_STATUSES+=(passed)
        printf 'PASS %s\n' "${id}"
    else
        GATE_STATUSES+=(failed)
        FAILURE_COUNT=$((FAILURE_COUNT + 1))
        printf 'FAIL %s (exit %s)\n' "${id}" "${status}" >&2
    fi
}

if [[ "${THREETERM_ACCEPTANCE_LIBRARY_ONLY:-false}" == true ]]; then
    if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
        return 0
    fi
    exit 0
fi

run_gate source-integrity \
    'git checkout is clean and its commit identity is readable' \
    bash -e -u -o pipefail -c '
        test "${SOURCE_COMMIT}" != unknown
        test "${SOURCE_CLEAN}" = true
    '

# The native prefixes have stable paths inside the disposable acceptance run.
# They are exported before Cargo gates start so every production test resolves
# the workers prepared by the native-worker gate rather than a system package.
export THREETERM_OCCT_DIR="${CARGO_TARGET_DIR}/native-workers/prefixes/occt"
export THREETERM_SLVS_DIR="${CARGO_TARGET_DIR}/native-workers/prefixes/slvs"
export THREETERM_OCCT_LIB_DIR="${THREETERM_OCCT_DIR}/lib"
export THREETERM_SLVS_LIB_DIR="${THREETERM_SLVS_DIR}/lib"
export LD_LIBRARY_PATH="${THREETERM_OCCT_LIB_DIR}:${THREETERM_SLVS_LIB_DIR}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
export THREETERM_REQUIRE_IMMUTABLE_WORKERS=1

run_gate worker.native \
    'immutable native-worker preparation, build, handshake, integration, and manifest' \
    bash -e -u -o pipefail -c '
        source "${ROOT}/.github/scripts/native-workers.sh"
        prepare_native_workers
        cargo build --workspace
        ACCEPTANCE_OCCT_WORKER="$(selected_worker_path occt)"
        ACCEPTANCE_SLVS_WORKER="$(selected_worker_path slvs)"
        test -x "${ACCEPTANCE_OCCT_WORKER}"
        test -x "${ACCEPTANCE_SLVS_WORKER}"
        export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${ACCEPTANCE_OCCT_WORKER}" | cut -d" " -f1)"
        export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${ACCEPTANCE_SLVS_WORKER}" | cut -d" " -f1)"
        verify_native_worker_execution "${ACCEPTANCE_OCCT_WORKER}" occt
        verify_native_worker_execution "${ACCEPTANCE_SLVS_WORKER}" slvs
        THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-occt-worker --test worker_integration \
            --jobs 1 -- --test-threads=1
        THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-occt-worker --test bracket_integration \
            --jobs 1 -- --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-host --test occt_geometry_smoke real_occt_geometry_smoke \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-host --test bracket_base_qualification \
            bracket_complete_recipe_qualifies_through_public_commands \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-slvs-worker --test real_worker \
            --jobs 1 -- --test-threads=1
        finalize_native_worker_manifest "${ACCEPTANCE_OCCT_WORKER}" "${ACCEPTANCE_SLVS_WORKER}" true
    '

catalog_worker_path() {
    local worker_id="$1"
    local candidate
    for candidate in "${CARGO_TARGET_DIR}/debug/build/threeterm-${worker_id}-"*/out/bin/threeterm-${worker_id}-worker; do
        if [[ -f "${candidate}" ]]; then
            printf '%s\n' "${candidate}"
            return 0
        fi
    done
    return 1
}

if ACCEPTANCE_OCCT_WORKER="$(catalog_worker_path occt 2>/dev/null)" && \
    ACCEPTANCE_SLVS_WORKER="$(catalog_worker_path slvs 2>/dev/null)"; then
    export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${ACCEPTANCE_OCCT_WORKER}" | cut -d' ' -f1)"
    export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${ACCEPTANCE_SLVS_WORKER}" | cut -d' ' -f1)"
else
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

run_gate baseline \
    'rustc pin, workspace check, format, and lint' \
    bash -e -u -o pipefail -c '
        expected_channel="$(tr -d "[:space:]" < rust-toolchain-channel.txt)"
        test "$(rustc --version | awk "{print \$2}")" = "${expected_channel}"
        cargo metadata --no-deps --format-version 1 >/dev/null
        cargo check --workspace
        cargo fmt --all -- --check
        cargo clippy --workspace --all-targets -- -D warnings
    '

run_gate workflow.l-bracket \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test l_bracket_workflow l_bracket_adapter_parity --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test l_bracket_workflow l_bracket_artifact_discard_replay --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-cli --test l_bracket_draft_e2e l_bracket_draft_commits_through_the_cli --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test mcp_bracket tools_call_to_bracket_produces_a_result_identical_to_the_cli_invocation --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test l_bracket_workflow l_bracket_adapter_parity \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test l_bracket_workflow l_bracket_artifact_discard_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-cli --test l_bracket_draft_e2e \
            l_bracket_draft_commits_through_the_cli \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test mcp_bracket \
            tools_call_to_bracket_produces_a_result_identical_to_the_cli_invocation \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.box-with-lid \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test box_lid_workflow box_lid_adapter_parity --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test box_lid_workflow box_lid_artifact_discard_replay --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-cli --test box_with_lid_e2e box_with_lid_runs_project_sketch_fit_extrude_viewport_export_reload --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test box_lid_workflow box_lid_adapter_parity \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test box_lid_workflow box_lid_artifact_discard_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-cli --test box_with_lid_e2e \
            box_with_lid_runs_project_sketch_fit_extrude_viewport_export_reload \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.reusable-component \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test component_instance reusable_geometry_artifact_discard_replay --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test component_instance cli_and_mcp_component_geometry_outcomes_match --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test component_instance reusable_geometry_artifact_discard_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test component_instance \
            cli_and_mcp_component_geometry_outcomes_match \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.historical-edit \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test historical_recovery_parity successful_historical_edit_has_equivalent_current_geometry_through_all_adapters --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-cli --test historical_recovery_e2e historical_failure_and_named_restore_use_the_production_cli_path --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test historical_recovery_parity \
            successful_historical_edit_has_equivalent_current_geometry_through_all_adapters \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-cli --test historical_recovery_e2e \
            historical_failure_and_named_restore_use_the_production_cli_path \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.object-timeline \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test historical_recovery_parity object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-cli --test object_specific_timeline_e2e feature_timeline_browsing_and_restore_use_the_production_cli_path --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test mcp_bracket tools_list_and_call_expose_the_feature_scoped_timeline_contract --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test historical_recovery_parity \
            object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-cli --test object_specific_timeline_e2e \
            feature_timeline_browsing_and_restore_use_the_production_cli_path \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test mcp_bracket \
            tools_list_and_call_expose_the_feature_scoped_timeline_contract \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.keyboard-first \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-tui --test production_launch production_launch_completes_keyboard_first_modeling_workflow_end_to_end --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-cli --test extrude_e2e generation_publication --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test mcp_bracket tools_call_to_bracket_produces_a_result_identical_to_the_cli_invocation --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-tui --test production_launch \
            production_launch_completes_keyboard_first_modeling_workflow_end_to_end \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-cli --test extrude_e2e generation_publication \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test mcp_bracket \
            tools_call_to_bracket_produces_a_result_identical_to_the_cli_invocation \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.invalid-edit-recovery \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test historical_recovery_parity historical_recovery_adapter_parity --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-cli --test historical_recovery_e2e historical_failure_and_named_restore_use_the_production_cli_path --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test invalid_recovery_parity invalid_geometry_preserves_canonical_state_and_matches_all_adapter_diagnostics --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test historical_recovery_parity historical_recovery_adapter_parity \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-cli --test historical_recovery_e2e \
            historical_failure_and_named_restore_use_the_production_cli_path \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test invalid_recovery_parity \
            invalid_geometry_preserves_canonical_state_and_matches_all_adapter_diagnostics \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate replay.canonical-extrude \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-host --test occt_atomicity canonical_extrude_replay --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-host --test occt_atomicity canonical_extrude_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate replay.boolean-pattern \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test domain_command_parity cli_mcp_and_tui_commit_and_replay_equivalent_boolean_patterns --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test domain_command_parity \
            cli_mcp_and_tui_commit_and_replay_equivalent_boolean_patterns \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate replay.reattach-edge \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test domain_command_parity cli_mcp_and_tui_commit_and_replay_equivalent_fillet_reattachments --jobs 1 -- --include-ignored --exact --test-threads=1
THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test domain_command_parity cli_mcp_and_tui_commit_and_replay_equivalent_split_reattachments --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test domain_command_parity \
            cli_mcp_and_tui_commit_and_replay_equivalent_fillet_reattachments \
            --jobs 1 -- --include-ignored --exact --test-threads=1
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test domain_command_parity \
            cli_mcp_and_tui_commit_and_replay_equivalent_split_reattachments \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate registry.command-schemas \
    'cargo test -p threeterm-protocol --test registry_shape --test registry_hash --test registry_bracket --jobs 1 -- --test-threads=1
cargo test -p threeterm-mcp --test mcp_bracket tools_list_advertises_every_registered_command_with_populated_schemas --jobs 1 -- --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        cargo test -p threeterm-protocol --test registry_shape --test registry_hash --test registry_bracket \
            --jobs 1 -- --test-threads=1
        cargo test -p threeterm-mcp --test mcp_bracket \
            tools_list_advertises_every_registered_command_with_populated_schemas \
            --jobs 1 -- --exact --test-threads=1
    '

run_gate schema.identities \
    'cargo run -p threeterm-cli --bin threeterm -- --machine new-project target/acceptance-run/schema-project' \
    bash -e -u -o pipefail -c '
        rm -rf -- "${SCHEMA_PROJECT}"
        cargo run -p threeterm-cli --bin threeterm -- --machine new-project "${SCHEMA_PROJECT}" \
            >"${SCHEMA_RESPONSE}"
        jq -e \
            ".manifest.schema_version and .manifest.command_registry_hash and \
             .manifest.feature_schema_version and .manifest.protocol_schema_version and \
             .manifest.occt_worker.worker_schema_version and .manifest.slvs_worker.worker_schema_version" \
            "${SCHEMA_RESPONSE}" >/dev/null
        test -f "${SCHEMA_PROJECT}/manifest.json"
    '

run_gate licensing.libslvs \
    'libslvs source policy and staged artifact verification' \
    bash -e -u -o pipefail -c '
        source "${ROOT}/.github/scripts/licensing.sh"
        verify_libslvs_source "${ROOT}"
        verify_libslvs_artifact "${LIBSLVS_ARTIFACT}/manifest.json" "${LIBSLVS_ARTIFACT}"
    '

run_gate release.namespace \
    'release.sh verify' \
    bash -e -u -o pipefail -c '
        "${ROOT}/.github/scripts/release.sh" verify
    '

run_gate documentation.workspace \
    'workspace packages, production entry points, and README documentation agree' \
    bash -e -u -o pipefail -c '
        expected_members=(
            threeterm-host threeterm-occt-worker threeterm-slvs-worker threeterm-tui
            threeterm-cli threeterm-mcp threeterm-viewport threeterm-persistence
            threeterm-theme threeterm-lua-bridge threeterm-domain threeterm-protocol rehearsal
        )
        actual_members="$(cargo metadata --no-deps --format-version 1 | jq -r ".packages[].name" | sort)"
        test "$(wc -l <<<"${actual_members}" | tr -d " ")" -eq "${#expected_members[@]}"
        for member in "${expected_members[@]}"; do
            grep -Fxq "${member}" <<<"${actual_members}"
            grep -Fq "${member}" README.md
        done
        grep -Fq "crates/rehearsal" README.md
        grep -Fq ".github/scripts/acceptance.sh" README.md
        grep -Fq "direct-Ghostty" README.md
        grep -Fq "Project Manifest" README.md
        grep -Fq "Official Interactive Environment" README.md
        grep -Fq "pinned rootless Arch image" README.md
        grep -Fq "xterm-ghostty/1.3.1-arch2" README.md
        grep -Fq "threeterm-mcp" README.md
        grep -Fq "\`threeterm-tui\` owns that interactive surface" README.md
        grep -Fq "Headless Automation adapters" README.md
        grep -Fq "CLI and MCP do not provide a graphical viewport" README.md
        grep -Fq "bash .github/scripts/acceptance.sh" README.md
        grep -Fq "PODMAN_ROOTLESS: \"1\"" .github/workflows/e2e.yml
        grep -Fq "docker.io/archlinux@sha256:b860afd5823683f7ea389ba5f00d812f4fe55f6f286dea329d2abeefa535e309" .github/workflows/e2e.yml
        grep -Fq "cargo run -p threeterm-cli --bin threeterm" docs/research/rehearsal-evidence/README.md
        grep -Fq -- "--machine rehearse" docs/research/rehearsal-evidence/README.md
        grep -Fq "threeterm-tui" crates/tui/Cargo.toml
        grep -Fq "name = \"threeterm\"" crates/cli/Cargo.toml
        grep -Fq "name = \"threeterm-mcp\"" crates/mcp/Cargo.toml
        test -f crates/cli/src/main.rs
        test -f crates/mcp/src/main.rs
        test -f crates/tui/src/bin/threeterm-tui.rs
    '

run_gate performance.claims \
    'verify_performance_material against the checked-in six-gate evidence' \
    bash -e -u -o pipefail -c '
        source "${ROOT}/.github/scripts/performance-gate.sh"
        material="${THREETERM_RELEASE_MATERIAL:-}"
        tag="${THREETERM_RELEASE_TAG:-}"
        if [[ -n "${material}" ]]; then
            test -n "${tag}"
            verify_performance_material "${ROOT}" "${material}" \
                "${ROOT}/docs/release/six-gate-performance-claims-gate.md" \
                "$(git -C "${ROOT}" rev-parse HEAD)" "${tag}"
        else
            printf "%s\n" "no performance claim material supplied; no target admitted"
        fi
    '

SOURCE_COMMIT_AFTER="$(git rev-parse HEAD 2>/dev/null || printf '%s' unknown)"
SOURCE_CHANGED=false
SOURCE_CLEAN_AFTER=true
SOURCE_STATUS_AFTER=''
if ! SOURCE_STATUS_AFTER="$(git status --porcelain --untracked-files=all 2>/dev/null)"; then
    SOURCE_CLEAN_AFTER=false
elif [[ -n "${SOURCE_STATUS_AFTER}" ]]; then
    SOURCE_CLEAN_AFTER=false
fi
if [[ "${SOURCE_COMMIT_AFTER}" != "${SOURCE_COMMIT}" || "${SOURCE_CLEAN_AFTER}" != true ]]; then
    SOURCE_CHANGED=true
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

relative_artifact_path() {
    local path="$1"
    local relative
    relative="$(realpath --relative-to="${ROOT}" "${path}" 2>/dev/null || true)"
    [[ -n "${relative}" && "${relative}" != .. && "${relative}" != ../* ]] || return 1
    printf '%s\n' "${relative}"
}

declare -a ARTIFACT_PATHS=()
add_artifact() {
    local path="$1"
    [[ -f "${path}" && ! -L "${path}" ]] || return 0
    ARTIFACT_PATHS+=("${path}")
}

for log in "${GATE_LOGS[@]}"; do
    add_artifact "${log}"
done
add_artifact "${NATIVE_MANIFEST}"
add_artifact "${OCCT_SMOKE_EVIDENCE}"
add_artifact "${LIBSLVS_ARTIFACT}/${ARTIFACT_MANIFEST_RELATIVE##*/}"
add_artifact "${SCHEMA_RESPONSE}"
add_artifact "${SCHEMA_PROJECT}/manifest.json"
add_artifact "${SCHEMA_PROJECT}/transactions.log"
add_artifact "${ROOT}/README.md"
add_artifact "${ROOT}/Cargo.toml"
add_artifact "${ROOT}/Cargo.lock"
add_artifact "${ROOT}/rust-toolchain-channel.txt"
add_artifact "${ROOT}/docs/release/trademark-and-namespace-gate.md"
add_artifact "${ROOT}/docs/release/six-gate-performance-claims-gate.md"
add_artifact "${ROOT}/docs/release/performance-claim-limitations.md"
add_artifact "${ROOT}/docs/research/rehearsal-evidence/README.md"
while IFS= read -r -d '' artifact; do
    add_artifact "${artifact}"
done < <(find "${LIBSLVS_ARTIFACT}" -type f -print0 2>/dev/null || true)
while IFS= read -r -d '' artifact; do
    add_artifact "${artifact}"
done < <(find "${ROOT}/docs/research/rehearsal-evidence" -type f -print0 2>/dev/null || true)
if [[ -f "${NATIVE_MANIFEST}" ]]; then
    while IFS= read -r worker; do
        add_artifact "${worker}"
    done < <(jq -r '.workers[]?.executable.path // empty' "${NATIVE_MANIFEST}" 2>/dev/null || true)
fi

ARTIFACTS='[]'
for path in "${ARTIFACT_PATHS[@]}"; do
    relative="$(relative_artifact_path "${path}" || true)"
    [[ -n "${relative}" ]] || continue
    bytes="$(wc -c <"${path}" | tr -d ' ')"
    digest="$(sha256sum "${path}" | cut -d' ' -f1)"
    ARTIFACTS="$(jq -c --arg path "${relative}" --argjson bytes "${bytes}" --arg sha256 "${digest}" \
        '. + [{path: $path, bytes: $bytes, sha256: $sha256}]' <<<"${ARTIFACTS}")"
done
ARTIFACTS="$(jq -c 'unique_by(.path) | sort_by(.path)' <<<"${ARTIFACTS}")"

json_field() {
    local file="$1"
    local filter="$2"
    local value
    value="$(jq -er "${filter}" "${file}" 2>/dev/null || true)"
    printf '%s\n' "${value:-unknown}"
}

SCHEMAS='{}'
if [[ -f "${SCHEMA_RESPONSE}" ]]; then
    project_manifest="${SCHEMA_PROJECT}/manifest.json"
    if [[ -f "${project_manifest}" ]]; then
        project_schema="$(json_field "${project_manifest}" '.schema_version')"
        registry_hash="$(json_field "${project_manifest}" '.command_registry_hash')"
        feature_schema="$(json_field "${project_manifest}" '.feature_schema_version')"
        protocol_schema="$(json_field "${project_manifest}" '.protocol_schema_version')"
        occt_schema="$(json_field "${project_manifest}" '.occt_worker.worker_schema_version')"
        slvs_schema="$(json_field "${project_manifest}" '.slvs_worker.worker_schema_version')"
        SCHEMAS="$(jq -cn --arg project "${project_schema}" --arg registry "${registry_hash}" \
            --arg feature "${feature_schema}" --arg protocol "${protocol_schema}" \
            --arg occt "${occt_schema}" --arg slvs "${slvs_schema}" \
            --arg native "$(json_field "${NATIVE_MANIFEST}" '.schema_version')" \
            --arg artifact "$(json_field "${LIBSLVS_ARTIFACT}/manifest.json" '.schema_version')" \
            '{project_manifest: $project, command_registry: $registry, feature: $feature,
              protocol: $protocol, occt_worker: $occt, slvs_worker: $slvs,
              native_worker_manifest: $native, libslvs_artifact: $artifact}')"
    fi
fi

WORKERS='{}'
if [[ -f "${NATIVE_MANIFEST}" ]]; then
    if WORKERS="$(jq -c '
        .workers
        | to_entries
        | map({key: .key, value: (.value + {manifest_key: .key})})
        | from_entries
    ' "${NATIVE_MANIFEST}" 2>/dev/null)"; then
        :
    else
        WORKERS='{}'
    fi
fi

EVIDENCE_VALID=true
NATIVE_MANIFEST_VERIFIED=false
if source "${ROOT}/.github/scripts/native-workers.sh"; then
    set +e
    if verify_native_worker_manifest "${NATIVE_MANIFEST}" "${LIBSLVS_ARTIFACT}"; then
        NATIVE_MANIFEST_VERIFIED=true
    fi
else
    set +e
fi
if [[ ! -f "${NATIVE_MANIFEST}" ]] || ! jq -e '
    .schema_version == "threeterm.ci.native-workers/2" and
    (.workers.occt.executed == true) and
    (.workers.libslvs.executed == true) and
    (.workers.occt.worker_id == "occt") and
    (.workers.libslvs.worker_id == "slvs") and
    (.workers.occt.worker_schema_version == "threeterm.workers.occt/1") and
    (.workers.libslvs.worker_schema_version == "threeterm.workers.slvs/1") and
    (.workers.occt.protocol_schema_version == "threeterm.protocol/1") and
    (.workers.libslvs.protocol_schema_version == "threeterm.protocol/1") and
    (.workers.occt.source_commit | test("^[0-9a-f]{40}$")) and
    (.workers.libslvs.source_commit | test("^[0-9a-f]{40}$")) and
    (.workers.occt.executable.path | type == "string") and
    (.workers.libslvs.executable.path | type == "string") and
    (.workers.occt.executable.sha256 | test("^[0-9a-f]{64}$")) and
    (.workers.libslvs.executable.sha256 | test("^[0-9a-f]{64}$")) and
    (.workers.occt.linked_libraries | type == "array" and length > 0 and all(.[]; (.path | type == "string") and (.sha256 | test("^[0-9a-f]{64}$")))) and
    (.workers.libslvs.linked_libraries | type == "array" and length > 0 and all(.[]; (.path | type == "string") and (.sha256 | test("^[0-9a-f]{64}$"))))
    ' "${NATIVE_MANIFEST}" >/dev/null 2>&1; then
    EVIDENCE_VALID=false
fi
if [[ ! -f "${OCCT_SMOKE_EVIDENCE}" ]] || ! jq -e '
    .worker.source_repository == $source_repository and
    .worker.source_commit == $source_commit and
    .worker.worker_schema_version == $worker_schema and
    .worker.protocol_schema_version == $protocol_schema and
    .schema_version == "threeterm.smoke.real-occt/1" and
    .test == "real_occt_geometry_smoke" and
    (.worker.path | type == "string" and length > 0) and
    (.worker.sha256 | test("^[0-9a-f]{64}$")) and
    (.worker.source_commit | type == "string" and length == 40) and
    (.kernel.linked_libraries | type == "array" and length > 0) and
    (.kernel.occt_libraries | type == "array" and length > 0) and
    all(.kernel.linked_libraries[]; (.path | type == "string" and startswith("/")) and (.sha256 | test("^[0-9a-f]{64}$"))) and
    all(.kernel.occt_libraries[]; (.path | type == "string" and startswith("/")) and (.sha256 | test("^[0-9a-f]{64}$")))
    ' --arg source_repository "${EXPECTED_OCCT_SOURCE_REPOSITORY}" \
      --arg source_commit "${EXPECTED_OCCT_SOURCE_COMMIT}" \
      --arg worker_schema "${EXPECTED_OCCT_WORKER_SCHEMA}" \
      --arg protocol_schema "${EXPECTED_PROTOCOL_SCHEMA}" \
      "${OCCT_SMOKE_EVIDENCE}" >/dev/null 2>&1; then
    EVIDENCE_VALID=false
fi
if [[ "${NATIVE_MANIFEST_VERIFIED}" != true ]]; then
    EVIDENCE_VALID=false
fi
if [[ ! -f "${SCHEMA_RESPONSE}" ]] || [[ ! -f "${SCHEMA_PROJECT}/manifest.json" ]] || \
    ! jq -e '
    (.schema_version | type == "string" and length > 0) and
    (.command_registry_hash | type == "string" and length > 0) and
    (.feature_schema_version | type == "string" and length > 0) and
    (.protocol_schema_version | type == "string" and length > 0) and
    (.occt_worker.worker_schema_version | type == "string" and length > 0) and
    (.slvs_worker.worker_schema_version | type == "string" and length > 0)
    ' "${SCHEMA_PROJECT}/manifest.json" >/dev/null 2>&1; then
    EVIDENCE_VALID=false
fi
if [[ "${SCHEMAS}" == '{}' ]] || ! jq -e '
    all([.project_manifest, .command_registry, .feature, .protocol,
         .occt_worker, .slvs_worker, .native_worker_manifest,
         .libslvs_artifact][]; type == "string" and length > 0 and . != "unknown")
    ' <<<"${SCHEMAS}" >/dev/null 2>&1; then
    EVIDENCE_VALID=false
fi
if [[ "${WORKERS}" == '{}' ]] || ! jq -e '
    (.occt.worker_id == "occt") and
    (.libslvs.worker_id == "slvs") and
    (.occt.manifest_key == "occt") and
    (.libslvs.manifest_key == "libslvs") and
    (.occt.worker_schema_version == "threeterm.workers.occt/1") and
    (.libslvs.worker_schema_version == "threeterm.workers.slvs/1") and
    (.occt.protocol_schema_version == "threeterm.protocol/1") and
    (.libslvs.protocol_schema_version == "threeterm.protocol/1") and
    (.occt.source_commit | test("^[0-9a-f]{40}$")) and
    (.libslvs.source_commit | test("^[0-9a-f]{40}$")) and
    (.occt.executable.sha256 | test("^[0-9a-f]{64}$")) and
    (.libslvs.executable.sha256 | test("^[0-9a-f]{64}$"))
    ' <<<"${WORKERS}" >/dev/null 2>&1; then
    EVIDENCE_VALID=false
fi
for evidence_path in "${NATIVE_MANIFEST}" "${OCCT_SMOKE_EVIDENCE}" "${LIBSLVS_ARTIFACT}/manifest.json" \
    "${SCHEMA_RESPONSE}" "${SCHEMA_PROJECT}/manifest.json"; do
    evidence_relative="$(relative_artifact_path "${evidence_path}" || true)"
    if [[ -z "${evidence_relative}" ]] || ! jq -e --arg path "${evidence_relative}" '
        any(.[]; .path == $path and (.bytes | type == "number" and . >= 0)
            and (.sha256 | test("^[0-9a-f]{64}$")))
        ' <<<"${ARTIFACTS}" >/dev/null 2>&1; then
        EVIDENCE_VALID=false
    fi
done
if [[ "${EVIDENCE_VALID}" != true ]]; then
    FAILURE_COUNT=$((FAILURE_COUNT + 1))
fi

GATES='[]'
for index in "${!GATE_IDS[@]}"; do
    log_relative="$(relative_artifact_path "${GATE_LOGS[${index}]}" || printf '%s' unknown)"
    output_json="$(jq -cn \
        --arg path "${log_relative}" \
        --argjson bytes "${GATE_LOG_BYTES[$index]:-0}" \
        --arg sha256 "${GATE_LOG_SHA256[${index}]}" \
        '{path: $path, bytes: $bytes, sha256: $sha256}')"
    GATES="$(jq -c \
        --arg id "${GATE_IDS[${index}]}" \
        --arg command "${GATE_COMMANDS[${index}]}" \
        --arg status "${GATE_STATUSES[${index}]}" \
        --argjson exit_status "${GATE_EXITS[${index}]}" \
        --argjson timed_out "${GATE_TIMED_OUT[$index]:-false}" \
        --argjson duration_ms "${GATE_DURATIONS_MS[${index}]:-0}" \
        --argjson output "${output_json}" \
        '. + [{id: $id, command: $command, status: $status,
               exit_status: $exit_status, timed_out: $timed_out,
               duration_ms: $duration_ms, output: $output,
               output_path: $output.path, output_sha256: $output.sha256}]' <<<"${GATES}")"
done

WORKFLOWS="$(jq -c '[.[] | select(.id | startswith("workflow."))]' <<<"${GATES}")"
CATALOG_TMP="${CATALOG}.tmp.$$"
CATALOG_RESULT=passed
if (( FAILURE_COUNT > 0 )) || [[ "${SOURCE_CHANGED}" == true ]]; then
    CATALOG_RESULT=failed
fi

jq -S -n \
    --arg schema_version threeterm.acceptance.catalog/1 \
    --arg repository https://github.com/rafaelromao/threeterm \
    --arg source_commit "${SOURCE_COMMIT}" \
    --arg source_commit_after "${SOURCE_COMMIT_AFTER}" \
    --argjson source_clean "${SOURCE_CLEAN}" \
    --argjson source_clean_after "${SOURCE_CLEAN_AFTER}" \
    --argjson source_changed "${SOURCE_CHANGED}" \
    --argjson evidence_complete "${EVIDENCE_VALID}" \
    --arg result "${CATALOG_RESULT}" \
    --argjson gates "${GATES}" \
    --argjson workflows "${WORKFLOWS}" \
    --argjson schemas "${SCHEMAS}" \
    --argjson workers "${WORKERS}" \
    --argjson artifacts "${ARTIFACTS}" \
    '{schema_version: $schema_version,
      source: {repository: $repository, commit: $source_commit,
               commit_after: $source_commit_after, clean: $source_clean,
               clean_after: $source_clean_after,
               changed_during_run: $source_changed},
      evidence: {complete: $evidence_complete},
      source_commit: $source_commit,
      schemas: $schemas,
      registry_hash: $schemas.command_registry,
      workers: $workers,
      gates: $gates,
      workflows: $workflows,
      artifacts: $artifacts,
      result: $result}' >"${CATALOG_TMP}"

if jq -e \
    '.schema_version == "threeterm.acceptance.catalog/1" and
     (.source.commit | type == "string") and
     (.evidence.complete | type == "boolean") and
     (.schemas | type == "object") and
     (.gates | length > 0) and
     (.gates | all(.status == "passed" or .status == "failed")) and
     (.gates | all((.timed_out | type == "boolean") and
                   (.duration_ms | type == "number" and . >= 0) and
                   (.output_path | type == "string") and
                   (.output_sha256 | test("^[0-9a-f]{64}$")))) and
     (.artifacts | all((.path | type == "string") and (.sha256 | test("^[0-9a-f]{64}$")))) and
     (.result == "passed" or .result == "failed")' "${CATALOG_TMP}" >/dev/null; then
    if ! mv -f -- "${CATALOG_TMP}" "${CATALOG}"; then
        printf '%s\n' 'acceptance catalog: unable to publish catalog atomically' >&2
        rm -f -- "${CATALOG_TMP}"
        exit 1
    fi
else
    printf '%s\n' 'acceptance catalog: generated JSON failed validation' >&2
    rm -f -- "${CATALOG_TMP}"
    exit 1
fi

printf 'Acceptance catalog: %s\n' "${CATALOG}"
printf 'Acceptance result: %s\n' "${CATALOG_RESULT}"
if (( FAILURE_COUNT > 0 )) || [[ "${SOURCE_CHANGED}" == true ]]; then
    exit 1
fi
