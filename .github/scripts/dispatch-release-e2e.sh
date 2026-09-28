#!/usr/bin/env bash
set -euo pipefail

: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"

release_ref="$(gh api \
    "repos/${GITHUB_REPOSITORY}/pulls?state=open&per_page=100" \
    --jq '[.[]
        | select(.head.repo.full_name == env.GITHUB_REPOSITORY)
        | select(.head.ref | startswith("release-please--branches--"))
        | select(any(.labels[]?; .name == "autorelease: pending"))
        | .head.ref] | first // empty')"

if [[ -z "${release_ref}" ]]; then
    printf '%s\n' 'No pending Release Please PR; skipping release E2E dispatch.'
    exit 0
fi

for workflow in e2e.yml three-journey.yml; do
    gh api --method POST \
        "repos/${GITHUB_REPOSITORY}/actions/workflows/${workflow}/dispatches" \
        -f "ref=${release_ref}"
done

printf 'Dispatched native E2E workflows for %s\n' "${release_ref}"
