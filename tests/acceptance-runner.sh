#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export THREETERM_ACCEPTANCE_LIBRARY_ONLY=true
export THREETERM_ACCEPTANCE_SCOPE=full
export THREETERM_ACCEPTANCE_GATE_TIMEOUT_SECONDS=1
export THREETERM_ACCEPTANCE_GATE_KILL_GRACE_SECONDS=1

# A headless runner defers the graphical gate to the required three-journey
# workflow, but still executes and records release-namespace failures.
THREETERM_ACCEPTANCE_SCOPE=native bash -e -u -o pipefail -c '
    source "$1/.github/scripts/acceptance.sh"
    run_gate coverage.all-surfaces "must be deferred" false
    [[ "${#GATE_IDS[@]}" -eq 0 ]]
    run_gate release.namespace "must remain required" false
    [[ "${GATE_IDS[0]}" == release.namespace ]]
    [[ "${GATE_STATUSES[0]}" == failed ]]
    [[ "${FAILURE_COUNT}" -eq 1 ]]
' _ "${ROOT}"

# shellcheck source=/dev/null
source "${ROOT}/.github/scripts/acceptance.sh"
export TEST_ACCEPTANCE_LOG_ROOT="${LOG_ROOT}"

run_gate test.timeout \
    'child process is terminated with its timed-out gate' \
    bash -e -u -o pipefail -c '
        sleep 30 &
        child=$!
        printf "%s\n" "${child}" >"${TEST_ACCEPTANCE_LOG_ROOT}/descendant.pid"
        wait "${child}"
    '

timeout_child="$(tr -d '[:space:]' <"${LOG_ROOT}/descendant.pid")"
for _ in {1..20}; do
    if ! kill -0 "${timeout_child}" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
! kill -0 "${timeout_child}" 2>/dev/null
[[ "${GATE_STATUSES[0]}" == failed ]]
[[ "${GATE_EXITS[0]}" == 124 ]]
[[ "${GATE_TIMED_OUT[0]}" == true ]]
[[ "${GATE_DURATIONS_MS[0]}" =~ ^[1-9][0-9]*$ ]]
timeout_log="${GATE_LOGS[0]}"
timeout_bytes="$(wc -c <"${timeout_log}" | tr -d ' ')"
timeout_sha256="$(sha256sum "${timeout_log}" | cut -d' ' -f1)"
[[ "${GATE_LOG_BYTES[0]}" == "${timeout_bytes}" ]]
[[ "${GATE_LOG_SHA256[0]}" == "${timeout_sha256}" ]]
gate_metadata="$(jq -cn \
    --arg path "$(realpath --relative-to="${ROOT}" "${timeout_log}")" \
    --argjson bytes "${GATE_LOG_BYTES[0]}" \
    --arg sha256 "${GATE_LOG_SHA256[0]}" \
    '{path: $path, bytes: $bytes, sha256: $sha256}')"
jq -e \
    '(.path | type == "string" and length > 0) and
     (.bytes | type == "number" and . > 0) and
     (.sha256 | test("^[0-9a-f]{64}$"))' <<<"${gate_metadata}" >/dev/null

run_gate test.after 'a later gate still executes after timeout' true
[[ "${GATE_IDS[1]}" == test.after ]]
[[ "${GATE_STATUSES[1]}" == passed ]]
[[ "${GATE_TIMED_OUT[1]}" == false ]]

run_gate coverage.all-surfaces 'full acceptance still requires graphical coverage' true
[[ "${GATE_IDS[2]}" == coverage.all-surfaces ]]
[[ "${GATE_STATUSES[2]}" == passed ]]

printf '%s\n' 'acceptance runner contract satisfied'
