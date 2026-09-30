#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DISPATCHER="${ROOT}/.github/scripts/dispatch-release-e2e.sh"
TEMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEMP_ROOT}"' EXIT
export GH_CALL_LOG="${TEMP_ROOT}/calls"
export GITHUB_REPOSITORY=rafaelromao/threeterm
export GH_TOKEN=contract-token
export GITHUB_RUN_ID=12345
export GITHUB_SERVER_URL=https://github.com
export RELEASE_HEAD_SHA=release-head-sha
export RELEASE_BRANCH=release-please--branches--main--components--threeterm
export GH_RUN_CREATED_AT=2099-01-01T00:00:00Z
export THREETERM_RELEASE_E2E_POLL_SECONDS=1
export THREETERM_RELEASE_E2E_TIMEOUT_SECONDS=1

gh() {
    if [[ "$1" == api && "$2" == "repos/${GITHUB_REPOSITORY}/pulls?state=open&per_page=100" ]]; then
        jq -r "$4" <<<"${GH_PULLS_FIXTURE}"
        return
    fi
    if [[ "$1" == api && "$2" == --method && "$3" == POST ]]; then
        endpoint="$4"
        payload=''
        if [[ "$*" == *"--input -"* ]]; then
            payload="$(</dev/stdin)"
        fi
        printf '%s | %s\n' "$*" "${payload}" >>"${GH_CALL_LOG}"
        if [[ "${endpoint}" == *"/check-runs" ]]; then
            printf '%s\n' '{"id":777}'
        fi
        return
    fi
    if [[ "$1" == api && "$2" == repos/${GITHUB_REPOSITORY}/actions/workflows/*/runs\?* ]]; then
        printf '%s\n' "$*" >>"${GH_CALL_LOG}"
        case "$2" in
            *e2e.yml/runs*)
                jq -cn --arg sha "${RELEASE_HEAD_SHA}" --arg created "${GH_RUN_CREATED_AT}" \
                    --arg conclusion "${GH_E2E_CONCLUSION}" \
                    '{workflow_runs:[{id:101,head_sha:$sha,event:"workflow_dispatch",status:"completed",conclusion:$conclusion,created_at:$created,html_url:"https://github.com/rafaelromao/threeterm/actions/runs/101"}]}'
                ;;
            *three-journey.yml/runs*)
                jq -cn --arg sha "${RELEASE_HEAD_SHA}" --arg created "${GH_RUN_CREATED_AT}" \
                    --arg status "${GH_THREE_JOURNEY_STATUS:-completed}" \
                    --arg conclusion "${GH_THREE_JOURNEY_CONCLUSION}" \
                    '{workflow_runs:[{id:202,head_sha:$sha,event:"workflow_dispatch",status:$status,conclusion:(if $status == "completed" then $conclusion else null end),created_at:$created,html_url:"https://github.com/rafaelromao/threeterm/actions/runs/202"}]}'
                ;;
            *) printf 'unexpected workflow-runs endpoint: %s\n' "$2" >&2; return 2 ;;
        esac
        return
    fi
    if [[ "$1" == api && "$2" == repos/${GITHUB_REPOSITORY}/actions/runs/*/jobs\?* ]]; then
        printf '%s\n' "$*" >>"${GH_CALL_LOG}"
        jq -cn \
            --arg name 'Aggregate and publish evidence catalog' \
            --arg status "${GH_THREE_JOURNEY_GATE_STATUS:-completed}" \
            --arg conclusion "${GH_THREE_JOURNEY_GATE_CONCLUSION:-failure}" \
            '{jobs:[{name:$name,status:$status,conclusion:(if $status == "completed" then $conclusion else null end)}]}'
        return
    fi
    printf 'unexpected gh call: %s\n' "$*" >&2
    return 2
}
export -f gh

export GH_PULLS_FIXTURE='[]'
output="$(bash "${DISPATCHER}")"
[[ "${output}" == *"No pending Release Please PR"* ]]
[[ ! -s "${GH_CALL_LOG}" ]]

export GH_PULLS_FIXTURE='[{"head":{"ref":"feature/add-part","sha":"release-head-sha","repo":{"full_name":"rafaelromao/threeterm"}},"labels":[{"name":"autorelease: pending"}]}]'
output="$(bash "${DISPATCHER}")"
[[ "${output}" == *"No pending Release Please PR"* ]]
[[ ! -s "${GH_CALL_LOG}" ]]

export GH_PULLS_FIXTURE='[{"number":597,"head":{"ref":"release-please--branches--main--components--threeterm","sha":"release-head-sha","repo":{"full_name":"rafaelromao/threeterm"}},"labels":[{"name":"autorelease: pending"}]}]'
export GH_E2E_CONCLUSION=success
export GH_THREE_JOURNEY_CONCLUSION=success
bash "${DISPATCHER}"
grep -Fq 'repos/rafaelromao/threeterm/statuses/release-head-sha' "${GH_CALL_LOG}"
grep -Fq 'context=Release E2E gate' "${GH_CALL_LOG}"
grep -Fq 'state=pending' "${GH_CALL_LOG}"
grep -Fq 'state=success' "${GH_CALL_LOG}"
grep -Fq 'actions/workflows/e2e.yml/dispatches --input -' "${GH_CALL_LOG}"
grep -Fq '"expected_catalog_result":"passed"' "${GH_CALL_LOG}"
grep -Fq 'actions/workflows/three-journey.yml/dispatches -f ref=release-please--branches--main--components--threeterm' "${GH_CALL_LOG}"
grep -Fq 'event=workflow_dispatch' "${GH_CALL_LOG}"

: >"${GH_CALL_LOG}"
export GH_THREE_JOURNEY_CONCLUSION=failure
if bash "${DISPATCHER}" >/dev/null 2>&1; then
    printf '%s\n' 'a failed three-journey workflow must fail the release E2E gate' >&2
    exit 1
fi
grep -Fq 'context=Release E2E gate' "${GH_CALL_LOG}"
grep -Fq 'state=failure' "${GH_CALL_LOG}"

: >"${GH_CALL_LOG}"
export GH_THREE_JOURNEY_STATUS=queued
export GH_THREE_JOURNEY_GATE_STATUS=completed
export GH_THREE_JOURNEY_GATE_CONCLUSION=failure
if output="$(bash "${DISPATCHER}" 2>&1)"; then
    printf '%s\n' 'a completed aggregate job must fail the gate even while its workflow run is queued' >&2
    exit 1
fi
[[ "${output}" == *'journeys=failure'* ]]
grep -Fq 'actions/runs/202/jobs' "${GH_CALL_LOG}"
grep -Fq 'context=Release E2E gate' "${GH_CALL_LOG}"
grep -Fq 'state=failure' "${GH_CALL_LOG}"
