#!/usr/bin/env bash
# Local acceptance verifier. Mirrors the acceptance contract:
#   1. The pinned Rust toolchain matches rust-toolchain-channel.txt.
#   2. cargo metadata reports exactly the 13 documented member packages.
#   3. cargo check --workspace succeeds.
#   4. cargo fmt --all -- --check passes.
#   5. cargo clippy --workspace --all-targets -- -D warnings passes.
#   6. serialized cargo test --workspace passes.
#   7. the unsigned trademark and namespace release gate is refused, while a
#      complete fixture is accepted by the gate verifier.
#
# Exits non-zero on the first failure. Intended for humans and pre-PR
# inspection; CI uses ci.sh instead.

set -euo pipefail

cd "$(dirname "$0")/../.."

EXPECTED_CHANNEL="$(tr -d '[:space:]' < rust-toolchain-channel.txt)"
EXPECTED_MEMBERS=(
    "threeterm-host"
    "threeterm-occt-worker"
    "threeterm-slvs-worker"
    "threeterm-tui"
    "threeterm-cli"
    "threeterm-mcp"
    "threeterm-viewport"
    "threeterm-persistence"
    "threeterm-theme"
    "threeterm-lua-bridge"
    "threeterm-domain"
    "threeterm-protocol"
    "rehearsal"
)

echo "==> Verifying pinned Rust toolchain channel == ${EXPECTED_CHANNEL}"
ACTUAL_CHANNEL="$(rustc --version | awk '{print $2}')"
if [ "${ACTUAL_CHANNEL}" != "${EXPECTED_CHANNEL}" ]; then
    echo "FAIL: rustc ${ACTUAL_CHANNEL} != pinned ${EXPECTED_CHANNEL}" >&2
    exit 1
fi

echo "==> Verifying cargo metadata reports the 13 expected member packages"
ACTUAL_MEMBERS_RAW="$(cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[].name' | sort)"
ACTUAL_MEMBERS_COUNT="$(wc -l <<<"${ACTUAL_MEMBERS_RAW}" | tr -d ' ')"
ACTUAL_MEMBERS="$(grep -v '^$' <<<"${ACTUAL_MEMBERS_RAW}")"
if [ "${ACTUAL_MEMBERS_COUNT}" -ne "${#EXPECTED_MEMBERS[@]}" ]; then
    echo "FAIL: expected exactly ${#EXPECTED_MEMBERS[@]} workspace members, got ${ACTUAL_MEMBERS_COUNT}" >&2
    echo "---" >&2
    echo "${ACTUAL_MEMBERS}" >&2
    exit 1
fi
for member in "${EXPECTED_MEMBERS[@]}"; do
    if ! grep -Fxq "${member}" <<<"${ACTUAL_MEMBERS}"; then
        echo "FAIL: missing workspace member ${member}" >&2
        echo "---" >&2
        echo "${ACTUAL_MEMBERS}" >&2
        exit 1
    fi
done

export CARGO_TARGET_DIR="${PWD}/target"
# shellcheck source=/dev/null
source "${PWD}/.github/scripts/native-workers.sh"
echo "==> Resolving immutable OCCT and libslvs sources"
prepare_native_workers
echo "==> Building the selected native workers"
cargo build --workspace
ACCEPTANCE_OCCT_WORKER="$(selected_worker_path occt)"
ACCEPTANCE_SLVS_WORKER="$(selected_worker_path slvs)"
export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${ACCEPTANCE_OCCT_WORKER}" | cut -d' ' -f1)"
export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${ACCEPTANCE_SLVS_WORKER}" | cut -d' ' -f1)"
verify_native_worker_execution "${ACCEPTANCE_OCCT_WORKER}" occt
verify_native_worker_execution "${ACCEPTANCE_SLVS_WORKER}" slvs

echo "==> cargo check --workspace"
cargo check --workspace

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
# Native-worker tests are serialized to avoid resource contention, matching CI.
cargo test --workspace --jobs 1 -- --test-threads=1 2>&1 \
    | tee "${CARGO_TARGET_DIR}/native-worker-test.log"
test -s "${CARGO_TARGET_DIR}/native-worker-test.log"

echo "==> canonical real-worker integration tests"
THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-occt-worker --test worker_integration \
    --jobs 1 -- --test-threads=1
THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-occt-worker --test bracket_integration \
    --jobs 1 -- --test-threads=1
THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-slvs-worker --test real_worker \
    --jobs 1 -- --test-threads=1

finalize_native_worker_manifest "${ACCEPTANCE_OCCT_WORKER}" "${ACCEPTANCE_SLVS_WORKER}"

echo "==> trademark and namespace release-gate test"
bash tests/release-gate.sh

echo "==> native-worker evidence contract test"
bash tests/native-worker-contract.sh

echo "==> Acceptance contract satisfied"
