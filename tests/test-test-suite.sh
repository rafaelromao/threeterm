#!/usr/bin/env bash

set -euo pipefail

ROOT="$(dirname "$0")/.."
TEST_SUITE="${ROOT}/.github/scripts/test-suite.sh"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TEMP_DIR}"' EXIT

printf '%s\n' \
    '#!/usr/bin/env bash' \
    'printf "%s\n" "$*" > "${THREETERM_CARGO_ARGS}"' \
    > "${TEMP_DIR}/cargo"
chmod +x "${TEMP_DIR}/cargo"

run_selector() {
    PATH="${TEMP_DIR}:${PATH}" THREETERM_CARGO_ARGS="${TEMP_DIR}/args" \
        bash "${TEST_SUITE}" "$1"
    ACTUAL="$(<"${TEMP_DIR}/args")"
    if [ "${ACTUAL}" != "$2" ]; then
        echo "expected cargo $2, got $ACTUAL" >&2
        exit 1
    fi
}

run_selector fast "test --workspace --jobs 1 -- --test-threads=1"
run_selector slow "test --workspace --jobs 1 -- --ignored --test-threads=1"

if PATH="${TEMP_DIR}:${PATH}" bash "${TEST_SUITE}" unsupported >/dev/null 2>&1; then
    echo "unsupported suite unexpectedly succeeded" >&2
    exit 1
fi
