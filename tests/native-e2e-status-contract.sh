#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CHECK_STATUS="${ROOT}/.github/scripts/check-native-e2e-status.sh"

bash "${CHECK_STATUS}" failed 1 0
if bash "${CHECK_STATUS}" failed 0 0 >/dev/null 2>&1; then
    printf '%s\n' 'an unexpected passing acceptance catalog should fail expected-failure mode' >&2
    exit 1
fi
if bash "${CHECK_STATUS}" failed 1 1 >/dev/null 2>&1; then
    printf '%s\n' 'a failed full E2E suite must fail even when acceptance failure is expected' >&2
    exit 1
fi
bash "${CHECK_STATUS}" passed 0 0
if bash "${CHECK_STATUS}" passed 1 0 >/dev/null 2>&1; then
    printf '%s\n' 'a failed acceptance catalog should fail expected-pass mode' >&2
    exit 1
fi
if bash "${CHECK_STATUS}" passed 0 1 >/dev/null 2>&1; then
    printf '%s\n' 'a failed full E2E suite should fail expected-pass mode' >&2
    exit 1
fi
