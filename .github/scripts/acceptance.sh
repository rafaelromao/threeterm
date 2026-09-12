#!/usr/bin/env bash
# Commit-bound ThreeTerm production conformance catalog.
#
# Every gate runs in its own fail-fast subprocess. The parent keeps running so
# a failed gate never hides later evidence, then publishes the failure catalog
# before returning a non-zero status.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${ROOT}"

export CARGO_TARGET_DIR="${ROOT}/target/acceptance-run"
CATALOG="${THREETERM_ACCEPTANCE_CATALOG:-${ROOT}/target/acceptance-catalog.json}"
LOG_ROOT="${CARGO_TARGET_DIR}/logs"
NATIVE_MANIFEST="${CARGO_TARGET_DIR}/native-worker-manifest.json"
LIBSLVS_ARTIFACT="${CARGO_TARGET_DIR}/libslvs-artifact"
ARTIFACT_MANIFEST_RELATIVE='libslvs-artifact/manifest.json'
SCHEMA_PROJECT="${CARGO_TARGET_DIR}/schema-project"
SCHEMA_RESPONSE="${CARGO_TARGET_DIR}/schema-response.json"
mkdir -p "$(dirname "${CATALOG}")"
rm -rf -- "${CARGO_TARGET_DIR}"
mkdir -p "${LOG_ROOT}"

readonly CATALOG LOG_ROOT NATIVE_MANIFEST LIBSLVS_ARTIFACT ARTIFACT_MANIFEST_RELATIVE SCHEMA_PROJECT SCHEMA_RESPONSE
export ROOT SOURCE_COMMIT SOURCE_CLEAN LIBSLVS_ARTIFACT SCHEMA_PROJECT SCHEMA_RESPONSE

SOURCE_COMMIT="$(git rev-parse HEAD 2>/dev/null || printf '%s' unknown)"
SOURCE_CLEAN=true
if [[ -n "$(git status --porcelain --untracked-files=all 2>/dev/null)" ]]; then
    SOURCE_CLEAN=false
fi

declare -a GATE_IDS=()
declare -a GATE_COMMANDS=()
declare -a GATE_STATUSES=()
declare -a GATE_EXITS=()
declare -a GATE_LOGS=()
declare -a GATE_LOG_BYTES=()
declare -a GATE_LOG_SHA256=()
FAILURE_COUNT=0

run_gate() {
    local id="$1"
    local display="$2"
    shift 2
    local log="${LOG_ROOT}/${id//[^A-Za-z0-9_.-]/_}.log"
    local status

    printf 'command: %s\n' "${display}" >"${log}"
    if "$@" >>"${log}" 2>&1; then
        status=0
    else
        status=$?
    fi

    GATE_IDS+=("${id}")
    GATE_COMMANDS+=("${display}")
    GATE_EXITS+=("${status}")
    GATE_LOGS+=("${log}")
    GATE_LOG_BYTES+=("$(wc -c <"${log}" | tr -d ' ')")
    GATE_LOG_SHA256+=("$(sha256sum "${log}" | cut -d' ' -f1)")
    if [[ "${status}" -eq 0 ]]; then
        GATE_STATUSES+=(passed)
        printf 'PASS %s\n' "${id}"
    else
        GATE_STATUSES+=(failed)
        FAILURE_COUNT=$((FAILURE_COUNT + 1))
        printf 'FAIL %s (exit %s)\n' "${id}" "${status}" >&2
    fi
}

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
        THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-slvs-worker --test real_worker \
            --jobs 1 -- --test-threads=1
        finalize_native_worker_manifest "${ACCEPTANCE_OCCT_WORKER}" "${ACCEPTANCE_SLVS_WORKER}" true
    '

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
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test l_bracket_workflow l_bracket_artifact_discard_replay --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test l_bracket_workflow l_bracket_artifact_discard_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.box-with-lid \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test box_lid_workflow box_lid_artifact_discard_replay --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test box_lid_workflow box_lid_artifact_discard_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.reusable-component \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test component_instance reusable_geometry_artifact_discard_replay --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test component_instance reusable_geometry_artifact_discard_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.historical-edit \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test historical_recovery_parity successful_historical_edit_has_equivalent_current_geometry_through_all_adapters --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test historical_recovery_parity \
            successful_historical_edit_has_equivalent_current_geometry_through_all_adapters \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.object-timeline \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test historical_recovery_parity object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test historical_recovery_parity \
            object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.keyboard-first \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-tui --test production_launch production_launch_completes_keyboard_first_modeling_workflow_end_to_end --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-tui --test production_launch \
            production_launch_completes_keyboard_first_modeling_workflow_end_to_end \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate workflow.invalid-edit-recovery \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-mcp --test historical_recovery_parity historical_recovery_adapter_parity --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-mcp --test historical_recovery_parity historical_recovery_adapter_parity \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate replay.canonical-extrude \
    'THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-host --test occt_atomicity canonical_extrude_replay --jobs 1 -- --include-ignored --exact --test-threads=1' \
    bash -e -u -o pipefail -c '
        THREETERM_REQUIRE_OCCT=1 THREETERM_REQUIRE_REAL_WORKER=1 \
            cargo test -p threeterm-host --test occt_atomicity canonical_extrude_replay \
            --jobs 1 -- --include-ignored --exact --test-threads=1
    '

run_gate registry.command-schemas \
    'cargo test -p threeterm-protocol --test registry_shape --test registry_hash --test registry_bracket --jobs 1 -- --test-threads=1' \
    bash -e -u -o pipefail -c '
        cargo test -p threeterm-protocol --test registry_shape --test registry_hash --test registry_bracket \
            --jobs 1 -- --test-threads=1
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
        grep -Fq "cargo run -p threeterm-cli --bin threeterm" docs/research/rehearsal-evidence/README.md
        grep -Fq -- "--machine rehearse" docs/research/rehearsal-evidence/README.md
        grep -Fq "threeterm-tui" crates/tui/Cargo.toml
        test -f crates/cli/src/bin/threeterm.rs
        test -f crates/tui/src/bin/threeterm-tui.rs
    '

run_gate performance.claims \
    'verify_performance_material against the checked-in six-gate evidence' \
    bash -e -u -o pipefail -c '
        source "${ROOT}/.github/scripts/performance-gate.sh"
        material="${THREETERM_RELEASE_MATERIAL:-}"
        tag="${THREETERM_RELEASE_TAG:-}"
        test -n "${material}"
        test -n "${tag}"
        verify_performance_material "${ROOT}" "${material}" \
            "${ROOT}/docs/release/six-gate-performance-claims-gate.md" \
            "$(git -C "${ROOT}" rev-parse HEAD)" "${tag}"
    '

SOURCE_COMMIT_AFTER="$(git rev-parse HEAD 2>/dev/null || printf '%s' unknown)"
SOURCE_CHANGED=false
SOURCE_CLEAN_AFTER=true
if [[ -n "$(git status --porcelain --untracked-files=all 2>/dev/null)" ]]; then
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
        | map({key: .key, value: (.value + {worker_id: .key})})
        | from_entries
    ' "${NATIVE_MANIFEST}" 2>/dev/null)"; then
        :
    else
        WORKERS='{}'
    fi
fi

GATES='[]'
for index in "${!GATE_IDS[@]}"; do
    log_relative="$(relative_artifact_path "${GATE_LOGS[${index}]}" || printf '%s' unknown)"
    output_json="$(jq -cn \
        --arg path "${log_relative}" \
        --argjson bytes "${GATE_LOG_BYTES[${index]}:-0}" \
        --arg sha256 "${GATE_LOG_SHA256[${index}]}" \
        '{path: $path, bytes: $bytes, sha256: $sha256}')"
    GATES="$(jq -c \
        --arg id "${GATE_IDS[${index}]}" \
        --arg command "${GATE_COMMANDS[${index}]}" \
        --arg status "${GATE_STATUSES[${index}]}" \
        --argjson exit_status "${GATE_EXITS[${index}]}" \
        --argjson output "${output_json}" \
        '. + [{id: $id, command: $command, status: $status,
               exit_status: $exit_status, output: $output,
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
     (.schemas | type == "object") and
     (.gates | length > 0) and
     (.gates | all(.status == "passed" or .status == "failed")) and
     (.artifacts | all((.path | type == "string") and (.sha256 | test("^[0-9a-f]{64}$")))) and
     (.result == "passed" or .result == "failed")' "${CATALOG_TMP}" >/dev/null; then
    mv -f -- "${CATALOG_TMP}" "${CATALOG}"
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
