#!/usr/bin/env bash
# Fast ThreeTerm CI script. Invoked by .github/workflows/ci.yml and also
# runnable locally for inspection.
#
# Native-worker verification belongs to e2e.sh so pull requests do not wait for
# a source build of OCCT and libslvs. Each step exits non-zero on failure.

set -euo pipefail

cd "$(dirname "$0")/../.."

CHANNEL="$(tr -d '[:space:]' < rust-toolchain-channel.txt)"
echo "==> Pinned Rust toolchain channel: ${CHANNEL}"

if ! command -v rustup >/dev/null 2>&1; then
    echo "==> Installing rustup + pinned toolchain ${CHANNEL}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain "${CHANNEL}" \
              --profile minimal --component rustfmt --component clippy
    # shellcheck source=/dev/null
    source "${HOME}/.cargo/env"
fi

if ! rustup toolchain list 2>/dev/null | grep -q "^${CHANNEL}"; then
    echo "==> Installing toolchain ${CHANNEL} via rustup"
    rustup toolchain install "${CHANNEL}" \
        --profile minimal --component rustfmt --component clippy
fi

export RUSTUP_TOOLCHAIN="${CHANNEL}"
# Build scripts still compile their Rust boundary, but do not prepare a native
# worker. Native E2E sets up the immutable workers in e2e.sh.
export THREETERM_SKIP_OCCTBUILD=1

echo "==> cargo check --workspace"
cargo check --workspace

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> test-suite selector contract"
bash tests/test-test-suite.sh

echo "==> fast test suite"
bash .github/scripts/test-suite.sh fast

echo "==> CI contract satisfied"
