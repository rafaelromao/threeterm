#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DISPATCHER="${ROOT}/.github/scripts/dispatch-release-e2e.sh"
TEMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEMP_ROOT}"' EXIT
export GH_CALL_LOG="${TEMP_ROOT}/calls"
export GITHUB_REPOSITORY=rafaelromao/threeterm
export GH_TOKEN=contract-token

gh() {
    if [[ "$1" == api && "$2" == "repos/${GITHUB_REPOSITORY}/pulls?state=open&per_page=100" ]]; then
        jq -r "$4" <<<"${GH_FIXTURE}"
        return
    fi
    if [[ "$1" == api && "$2" == --method && "$3" == POST ]]; then
        printf '%s\n' "$*" >>"${GH_CALL_LOG}"
        return
    fi
    printf 'unexpected gh call: %s\n' "$*" >&2
    return 2
}
export -f gh

export GH_FIXTURE='[]'
output="$(bash "${DISPATCHER}")"
[[ "${output}" == *"No pending Release Please PR"* ]]
[[ ! -s "${GH_CALL_LOG}" ]]

export GH_FIXTURE='[{"head":{"ref":"feature/add-part","repo":{"full_name":"rafaelromao/threeterm"}},"labels":[{"name":"autorelease: pending"}]}]'
output="$(bash "${DISPATCHER}")"
[[ "${output}" == *"No pending Release Please PR"* ]]
[[ ! -s "${GH_CALL_LOG}" ]]

export GH_FIXTURE='[{"head":{"ref":"release-please--branches--main","repo":{"full_name":"rafaelromao/threeterm"}},"labels":[{"name":"autorelease: pending"}]}]'
bash "${DISPATCHER}"
[[ "$(wc -l <"${GH_CALL_LOG}" | tr -d ' ')" == 2 ]]
grep -Fq 'actions/workflows/e2e.yml/dispatches -f ref=release-please--branches--main' "${GH_CALL_LOG}"
grep -Fq 'actions/workflows/three-journey.yml/dispatches -f ref=release-please--branches--main' "${GH_CALL_LOG}"
