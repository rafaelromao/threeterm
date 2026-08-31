#!/usr/bin/env bash
# Canonical ThreeTerm CI script. Invoked by .github/workflows/ci.yml inside a
# rootless-Podman archlinux container; also runnable locally for inspection.
#
# The script is the single source of truth for the CI contract; the workflow
# YAML must not duplicate these commands. Each step exits non-zero on failure.

set -euo pipefail

cd "$(dirname "$0")/../.."

CHANNEL="$(tr -d '[:space:]' < rust-toolchain-channel.txt)"
echo "==> Pinned Rust toolchain channel: ${CHANNEL}"

if ! command -v rustc >/dev/null 2>&1; then
    echo "==> Installing rustup + pinned toolchain ${CHANNEL}"
    if command -v pacman >/dev/null 2>&1; then
        pacman -Syu --noconfirm rustup gcc
    else
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain "${CHANNEL}" \
                  --profile minimal --component rustfmt --component clippy
        # shellcheck source=/dev/null
        source "${HOME}/.cargo/env"
    fi
fi

if ! command -v cc >/dev/null 2>&1 && command -v pacman >/dev/null 2>&1; then
    echo "==> Installing gcc (C linker) via pacman"
    pacman -Syu --noconfirm gcc
fi

# Install the pinned OCCT development package so the disposable OCCT
# worker binary builds against the system library. Archlinux's
# community `opencascade` package ships the OCCT 7.x headers and
# runtime libraries the build.rs probes for.
if command -v pacman >/dev/null 2>&1; then
    echo "==> Installing opencascade (OCCT) via pacman"
    pacman -Syu --noconfirm opencascade
fi

if ! rustup toolchain list 2>/dev/null | grep -q "^${CHANNEL}"; then
    echo "==> Installing toolchain ${CHANNEL} via rustup"
    rustup toolchain install "${CHANNEL}" \
        --profile minimal --component rustfmt --component clippy
fi

echo "==> Activating toolchain ${CHANNEL}"
rustup default "${CHANNEL}" >/dev/null

echo "==> cargo check --workspace"
cargo check --workspace

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> test-suite selector contract"
bash tests/test-test-suite.sh

echo "==> fast test suite"
# OCCT integration tests spawn disposable native workers; serialize the test
# harness so the rootless CI container does not kill workers under fan-out.
THREETERM_REQUIRE_OCCT=1 bash .github/scripts/test-suite.sh fast

echo "==> trademark and namespace release-gate test"
bash tests/release-gate.sh

echo "==> CI contract satisfied"
