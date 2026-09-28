#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
    printf 'usage: %s <expected-catalog-result:passed|failed> <acceptance-exit> <e2e-exit>\n' \
        "${0##*/}" >&2
    exit 2
fi

expected_catalog_result="$1"
acceptance_status="$2"
e2e_status="$3"
for status in "${acceptance_status}" "${e2e_status}"; do
    if [[ ! "${status}" =~ ^[0-9]+$ ]]; then
        printf 'native E2E exit status must be a nonnegative integer: %s\n' "${status}" >&2
        exit 2
    fi
done

if [[ "${e2e_status}" -ne 0 ]]; then
    printf 'full native E2E suite failed with exit status %s\n' "${e2e_status}" >&2
    exit "${e2e_status}"
fi

case "${expected_catalog_result}" in
    passed)
        if [[ "${acceptance_status}" -ne 0 ]]; then
            printf 'expected a passing acceptance catalog, got exit status %s\n' \
                "${acceptance_status}" >&2
            exit "${acceptance_status}"
        fi
        ;;
    failed)
        if [[ "${acceptance_status}" -eq 0 ]]; then
            printf '%s\n' 'expected a failing acceptance catalog, but it passed' >&2
            exit 1
        fi
        ;;
    *)
        printf 'unknown expected catalog result: %s\n' "${expected_catalog_result}" >&2
        exit 2
        ;;
esac

printf 'Acceptance catalog matched expected %s result; full E2E passed.\n' \
    "${expected_catalog_result}"
