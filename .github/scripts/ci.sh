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

# Native dependencies are built from exact upstream commits by the immutable
# worker contract. Only build tooling is installed from the container package
# manager; mutable system OCCT/libslvs packages are never accepted.
if command -v pacman >/dev/null 2>&1; then
    echo "==> Installing native-worker build tooling via pacman"
    pacman -Syu --noconfirm --needed base-devel cmake git curl jq freetype2 fontconfig libx11
fi

if ! rustup toolchain list 2>/dev/null | grep -q "^${CHANNEL}"; then
    echo "==> Installing toolchain ${CHANNEL} via rustup"
    rustup toolchain install "${CHANNEL}" \
        --profile minimal --component rustfmt --component clippy
fi

echo "==> Activating toolchain ${CHANNEL}"
rustup default "${CHANNEL}" >/dev/null

export CARGO_TARGET_DIR="${PWD}/target"
# shellcheck source=/dev/null
source "${PWD}/.github/scripts/native-workers.sh"
echo "==> Resolving immutable OCCT and libslvs sources"
prepare_native_workers

echo "==> Building the selected native workers"
cargo build --workspace
OCCT_WORKER="$(selected_worker_path occt)"
SLVS_WORKER="$(selected_worker_path slvs)"
export THREETERM_OCCT_WORKER_SHA256="$(sha256sum "${OCCT_WORKER}" | cut -d' ' -f1)"
export THREETERM_SLVS_WORKER_SHA256="$(sha256sum "${SLVS_WORKER}" | cut -d' ' -f1)"
test -x "${OCCT_WORKER}" || { echo "OCCT worker is not executable: ${OCCT_WORKER}" >&2; exit 1; }
test -x "${SLVS_WORKER}" || { echo "libslvs worker is not executable: ${SLVS_WORKER}" >&2; exit 1; }
verify_native_worker_execution "${OCCT_WORKER}" occt
verify_native_worker_execution "${SLVS_WORKER}" slvs

echo "==> cargo check --workspace"
cargo check --workspace

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
# OCCT integration tests spawn disposable native workers; serialize the test
# harness so the rootless CI container does not kill workers under fan-out.
# This legacy flow assumes the optional system solver's sketch persistence
# behavior; the canonical worker tests below cover the approved solver.
cargo test --workspace --jobs 1 -- --test-threads=1 \
    --skip box_with_lid_runs_project_sketch_fit_extrude_viewport_export_reload 2>&1 \
    | tee "${CARGO_TARGET_DIR}/native-worker-test.log"
test -s "${CARGO_TARGET_DIR}/native-worker-test.log"

echo "==> canonical real-worker integration tests"
THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-occt-worker --test worker_integration \
    --jobs 1 -- --test-threads=1
THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-occt-worker --test bracket_integration \
    --jobs 1 -- --test-threads=1
THREETERM_REQUIRE_REAL_WORKER=1 cargo test -p threeterm-slvs-worker --test real_worker \
    --jobs 1 -- --test-threads=1

echo "==> Writing native-worker execution manifest"
finalize_native_worker_manifest "${OCCT_WORKER}" "${SLVS_WORKER}" true

echo "==> trademark and namespace release-gate test"
bash tests/release-gate.sh

echo "==> native-worker evidence contract test"
bash tests/native-worker-contract.sh

echo "==> CI contract satisfied"
