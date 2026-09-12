#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="${ROOT}/.github/scripts/acceptance.sh"

bash -n "${SCRIPT}"

for gate in \
    l_bracket_artifact_discard_replay \
    box_lid_artifact_discard_replay \
    reusable_geometry_artifact_discard_replay \
    successful_historical_edit_has_equivalent_current_geometry_through_all_adapters \
    object_timeline_adapter_parity_matches_registered_cli_mcp_and_tui_payloads \
    production_launch_completes_keyboard_first_modeling_workflow_end_to_end \
    historical_recovery_adapter_parity \
    canonical_extrude_replay; do
    count="$(grep -Fc "${gate}" "${SCRIPT}")"
    [[ "${count}" -eq 2 ]] || {
        printf 'expected one declared command and one executed command for %s, got %s\n' "${gate}" "${count}" >&2
        exit 1
    }
done

for required in \
    'threeterm.acceptance.catalog/1' \
    'native-worker-manifest.json' \
    'libslvs-artifact/manifest.json' \
    'sha256' \
    'source_commit' \
    'worker_id' \
    'schema_version' \
    'release.sh verify' \
    'verify_performance_material'; do
    grep -Fq "${required}" "${SCRIPT}" || {
        printf 'acceptance script is missing %s\n' "${required}" >&2
        exit 1
    }
done

for forbidden in \
    'tests/release-gate.sh' \
    'tests/release-artifacts.sh' \
    'tests/performance-gate.sh' \
    'tests/native-worker-contract.sh' \
    'tests/libslvs-artifact.sh' \
    'tests/libslvs-licensing.sh'; do
    if grep -Fq "${forbidden}" "${SCRIPT}"; then
        printf 'acceptance script uses test-only substitute %s\n' "${forbidden}" >&2
        exit 1
    fi
done

grep -Fq 'finalize_native_worker_manifest "${ACCEPTANCE_OCCT_WORKER}" "${ACCEPTANCE_SLVS_WORKER}" true' \
    "${SCRIPT}" || {
    printf '%s\n' 'acceptance script must record executed native workers' >&2
    exit 1
}

printf '%s\n' 'acceptance catalog contract satisfied'
