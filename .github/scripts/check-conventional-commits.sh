#!/usr/bin/env bash
set -euo pipefail

is_conventional_commit_header() {
    local header="$1"
    local pattern='^(feat|fix|perf|refactor|docs|build|ci|chore|test|revert)(\([^()]+\))?(!)?: [^[:space:]].*$'
    [[ "${header}" =~ ${pattern} ]]
}

check_commit_range() {
    local base_sha="$1"
    local head_sha="$2"
    local commits commit subject

    git rev-parse --verify "${base_sha}^{commit}" >/dev/null
    git rev-parse --verify "${head_sha}^{commit}" >/dev/null
    commits="$(git rev-list --no-merges "${base_sha}..${head_sha}")"
    if [[ -z "${commits}" ]]; then
        printf '%s\n' 'No non-merge commits to validate.'
        return 0
    fi

    while IFS= read -r commit; do
        subject="$(git show -s --format=%s "${commit}")"
        if ! is_conventional_commit_header "${subject}"; then
            printf 'Invalid commit header %s: %s\n' "${commit}" "${subject}" >&2
            printf '%s\n' 'Use type(scope): subject, with an allowed Conventional Commit type.' >&2
            return 1
        fi
    done <<<"${commits}"

    printf 'Validated %s non-merge Conventional Commit headers.\n' "$(wc -l <<<"${commits}" | tr -d ' ')"
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    if [[ $# -ne 2 ]]; then
        printf 'usage: %s <base-sha> <head-sha>\n' "${0##*/}" >&2
        exit 2
    fi
    check_commit_range "$1" "$2"
fi
