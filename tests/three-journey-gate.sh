#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GATE="${ROOT}/.github/scripts/three-journey-gate.sh"
RUN_ROOT="${ROOT}/target/three-journey-gate-contract"
MISSING_MANIFEST_ROOT="${ROOT}/target/three-journey-gate-missing-manifest"

trap 'rm -rf "${RUN_ROOT}" "${MISSING_MANIFEST_ROOT}"' EXIT
rm -rf "${RUN_ROOT}" "${MISSING_MANIFEST_ROOT}"
mkdir -p "${RUN_ROOT}"

bash -n "${GATE}"
help="$(bash "${GATE}" --help)"
for required in \
    '--surface NAME' \
    '--synthesize NAME STATUS REASON' \
    '--aggregate' \
    'threeterm-graphical' \
    'THREETERM_THREE_JOURNEY_TIMEOUT_SECONDS' \
    'THREETERM_THREE_JOURNEY_KILL_GRACE_SECONDS'; do
    grep -Fq -- "${required}" <<<"${help}"
done

for required in \
    'threeterm.acceptance.run/1' \
    'threeterm.acceptance.attempt/1' \
    'prerequisite_skipped' \
    'setsid --wait' \
    'kill -TERM -- "-$2"' \
    'journey gate timeout' \
    'THREETERM_JOURNEY_EVIDENCE_ROOT' \
    'THREETERM_COVERAGE_EVIDENCE_ROOT' \
    'threeterm-journey-gate'; do
    grep -Fq -- "${required}" "${GATE}"
done

set +e
mkdir -p "${RUN_ROOT}/process"
: >"${RUN_ROOT}/process/tui.stderr.timeout"
THREETERM_THREE_JOURNEY_ROOT="${RUN_ROOT}" \
THREETERM_JOURNEY_RUN_ID=contract-run \
THREETERM_THREE_JOURNEY_SKIP_NATIVE=true \
    bash "${GATE}" --surface tui >"${RUN_ROOT}/surface.stdout" 2>"${RUN_ROOT}/surface.stderr"
status=$?
set -e
((status != 0))
[[ ! -e "${RUN_ROOT}/process/tui.stderr.timeout" ]]
jq -e '
    .schema_version == "threeterm.acceptance.attempt/1" and
    .run_id == "contract-run" and
    .surface == "tui" and
    .status == "prerequisite_skipped" and
    .prerequisite.status == "skipped" and
    (.duration_ms | type == "number") and
    (.stderr.path | type == "string")
' "${RUN_ROOT}/attempts/tui.json" >/dev/null

THREETERM_THREE_JOURNEY_ROOT="${RUN_ROOT}" \
THREETERM_JOURNEY_RUN_ID=contract-run \
    bash "${GATE}" --synthesize api unrun 'headless job was cancelled'
jq -e '
    .surface == "api" and
    .status == "unrun" and
    .exit_status == null and
    .failure == "headless job was cancelled"
' "${RUN_ROOT}/attempts/api.json" >/dev/null

set +e
THREETERM_THREE_JOURNEY_ROOT="${RUN_ROOT}" \
THREETERM_JOURNEY_RUN_ID=contract-run \
    bash "${GATE}" --aggregate >"${RUN_ROOT}/aggregate.stdout" 2>"${RUN_ROOT}/aggregate.stderr"
aggregate_status=$?
set -e
((aggregate_status != 0))
jq -e '
    .schema_version == "threeterm.acceptance.three-journey/1" and
    .result == "failed" and
    (.journeys | length == 3) and
    any(.errors[]; contains("api") or contains("mcp") or contains("tui"))
' "${RUN_ROOT}/catalog.json" >/dev/null

set +e
THREETERM_THREE_JOURNEY_ROOT="${MISSING_MANIFEST_ROOT}" \
THREETERM_JOURNEY_RUN_ID=missing-manifest-run \
    bash "${GATE}" --aggregate >"${MISSING_MANIFEST_ROOT}.stdout" 2>"${MISSING_MANIFEST_ROOT}.stderr"
missing_manifest_status=$?
set -e
((missing_manifest_status != 0))
jq -e '
    .schema_version == "threeterm.acceptance.three-journey/1" and
    .result == "failed" and
    any(.errors[]; contains("attempt") or contains("coverage") or contains("geometric"))
' "${MISSING_MANIFEST_ROOT}/catalog.json" >/dev/null

printf '%s\n' 'three-journey gate contract satisfied'
