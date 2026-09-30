#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"
: "${GITHUB_RUN_ID:?GITHUB_RUN_ID is required}"
: "${GITHUB_SERVER_URL:?GITHUB_SERVER_URL is required}"

readonly STATUS_CONTEXT='Release E2E gate'
readonly POLL_SECONDS="${THREETERM_RELEASE_E2E_POLL_SECONDS:-15}"
readonly TIMEOUT_SECONDS="${THREETERM_RELEASE_E2E_TIMEOUT_SECONDS:-16200}"
if [[ ! "${POLL_SECONDS}" =~ ^[1-9][0-9]*$ || ! "${TIMEOUT_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
    printf '%s\n' 'Release E2E poll and timeout values must be positive integers' >&2
    exit 2
fi

release_pr_json="$(gh api \
    "repos/${GITHUB_REPOSITORY}/pulls?state=open&per_page=100" \
    --jq '[.[]
        | select(.head.repo.full_name == env.GITHUB_REPOSITORY)
        | select(.head.ref | startswith("release-please--branches--"))
        | select(any(.labels[]?; .name == "autorelease: pending"))
        | {number: .number, ref: .head.ref, sha: .head.sha}] | first // empty')"

if [[ -z "${release_pr_json}" ]]; then
    printf '%s\n' 'No pending Release Please PR; skipping release E2E dispatch.'
    exit 0
fi

release_number="$(jq -er '.number' <<<"${release_pr_json}")"
release_ref="$(jq -er '.ref' <<<"${release_pr_json}")"
release_sha="$(jq -er '.sha' <<<"${release_pr_json}")"
status_target_url="${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/actions/runs/${GITHUB_RUN_ID}"
status_started=false
status_completed=false

post_status() {
    local state="$1"
    local description="$2"
    gh api --method POST \
        "repos/${GITHUB_REPOSITORY}/statuses/${release_sha}" \
        -f "state=${state}" \
        -f "target_url=${status_target_url}" \
        -f "description=${description}" \
        -f "context=${STATUS_CONTEXT}" >/dev/null
}

on_exit() {
    local exit_status=$?
    if [[ "${status_started}" == true && "${status_completed}" != true ]]; then
        set +e
        if [[ "${exit_status}" -eq 0 ]]; then
            post_status success 'Native E2E and three-journey checks passed.'
        else
            post_status failure 'Release E2E dispatcher did not complete successfully.'
        fi
        set -e
    fi
    return "${exit_status}"
}
trap on_exit EXIT

post_status pending "Native E2E checks are running for release PR #${release_number}."
status_started=true

dispatch_workflow() {
    local workflow="$1"
    local endpoint="repos/${GITHUB_REPOSITORY}/actions/workflows/${workflow}/dispatches"
    local dispatched_at
    dispatched_at="$(date +%s)"
    if [[ "${workflow}" == e2e.yml ]]; then
        jq -cn --arg ref "${release_ref}" \
            '{ref: $ref, inputs: {expected_catalog_result: "passed"}}' \
            | gh api --method POST "${endpoint}" --input - >/dev/null
    else
        gh api --method POST "${endpoint}" -f "ref=${release_ref}" >/dev/null
    fi
    printf '%s\n' "${dispatched_at}"
}

wait_for_workflow() {
    local workflow="$1"
    local dispatched_at="$2"
    local deadline=$(( $(date +%s) + TIMEOUT_SECONDS ))
    local runs_json run_json jobs_json gate_job_json now status run_id gate_job_name
    local endpoint="repos/${GITHUB_REPOSITORY}/actions/workflows/${workflow}/runs?branch=${release_ref}&event=workflow_dispatch&per_page=100"

    case "${workflow}" in
        e2e.yml)
            gate_job_name='Native worker end-to-end checks'
            ;;
        three-journey.yml)
            gate_job_name='Aggregate and publish evidence catalog'
            ;;
        *)
            printf 'No terminal gate job is configured for %s\n' "${workflow}" >&2
            return 1
            ;;
    esac

    while :; do
        runs_json="$(gh api "${endpoint}")" || return 1
        run_json="$(jq -c --arg sha "${release_sha}" --argjson dispatched_at "${dispatched_at}" '
            [.workflow_runs[]?
             | select(.head_sha == $sha and .event == "workflow_dispatch")
             | select((.created_at | sub("\\.[0-9]+Z$"; "Z") | fromdateiso8601) >= $dispatched_at)]
            | sort_by(.created_at) | last // empty
        ' <<<"${runs_json}")" || return 1
        if [[ -n "${run_json}" ]]; then
            status="$(jq -r '.status' <<<"${run_json}")"
            if [[ "${status}" == completed ]]; then
                printf '%s\n' "${run_json}"
                return 0
            fi

            # A workflow can stay queued after its aggregate gate has finished
            # when an unrelated self-hosted job is still waiting for a runner.
            # The aggregate job is the authoritative result for this gate.
            run_id="$(jq -r '.id // empty' <<<"${run_json}")"
            if [[ -n "${run_id}" ]]; then
                jobs_json="$(gh api "repos/${GITHUB_REPOSITORY}/actions/runs/${run_id}/jobs?per_page=100")" || return 1
                gate_job_json="$(jq -c --arg name "${gate_job_name}" '
                    [.jobs[]? | select(.name == $name and .status == "completed")]
                    | last // empty
                ' <<<"${jobs_json}")" || return 1
                if [[ -n "${gate_job_json}" ]]; then
                    jq -cn \
                        --arg conclusion "$(jq -r '.conclusion // "missing"' <<<"${gate_job_json}")" \
                        --arg html_url "$(jq -r '.html_url // empty' <<<"${run_json}")" \
                        '{conclusion: $conclusion, html_url: $html_url}'
                    return 0
                fi
            fi
        fi
        now="$(date +%s)"
        if ((now >= deadline)); then
            printf 'Timed out waiting for %s on %s\n' "${workflow}" "${release_ref}" >&2
            return 1
        fi
        sleep "${POLL_SECONDS}"
    done
}

e2e_dispatched_at="$(dispatch_workflow e2e.yml)"
journey_dispatched_at="$(dispatch_workflow three-journey.yml)"

e2e_run_json=''
journey_run_json=''
if e2e_run_json="$(wait_for_workflow e2e.yml "${e2e_dispatched_at}")"; then :; fi
if journey_run_json="$(wait_for_workflow three-journey.yml "${journey_dispatched_at}")"; then :; fi

e2e_conclusion=missing
journey_conclusion=missing
e2e_url=''
journey_url=''
if [[ -n "${e2e_run_json}" ]]; then
    e2e_conclusion="$(jq -r '.conclusion // "missing"' <<<"${e2e_run_json}")"
    e2e_url="$(jq -r '.html_url // empty' <<<"${e2e_run_json}")"
fi
if [[ -n "${journey_run_json}" ]]; then
    journey_conclusion="$(jq -r '.conclusion // "missing"' <<<"${journey_run_json}")"
    journey_url="$(jq -r '.html_url // empty' <<<"${journey_run_json}")"
fi
if [[ "${e2e_conclusion}" == success && "${journey_conclusion}" == success ]]; then
    post_status success "Native E2E and three-journey checks passed for PR #${release_number}."
    status_completed=true
    printf 'Release E2E gate passed for PR #%s (native=%s, journeys=%s)\n' \
        "${release_number}" "${e2e_url}" "${journey_url}"
    exit 0
fi

post_status failure "Release E2E failed for PR #${release_number} (native=${e2e_conclusion}, journeys=${journey_conclusion})."
status_completed=true
printf 'Release E2E gate failed for PR #%s (native=%s, journeys=%s)\n' \
    "${release_number}" "${e2e_conclusion}" "${journey_conclusion}" >&2
exit 1
