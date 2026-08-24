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
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target}"

missing_dependency() {
    echo "missing dependency: $1 — expected $2 — remediation: $3" >&2
    exit 1
}

verify_pinned_environment() {
    local occt_version rust_version worker

    rust_version="$(rustc --version 2>&1)" || missing_dependency rust "${CHANNEL}" "rustup toolchain install ${CHANNEL}"
    [[ "${rust_version}" == "rustc ${CHANNEL}"* ]] \
        || missing_dependency rust "${CHANNEL}" "rustup toolchain install ${CHANNEL}"
    [[ "$(rustup default 2>/dev/null)" == "${CHANNEL}"* ]] \
        || missing_dependency rust "${CHANNEL}" "rustup default ${CHANNEL}"

    if command -v pacman >/dev/null 2>&1; then
        pacman -Qi opencascade >/dev/null 2>&1 \
            || missing_dependency opencascade "system OCCT (pinned V7_9_2 source contract)" "pacman -S opencascade"
        occt_version="$(pacman -Q opencascade | cut -d ' ' -f 2)"
        [[ "${occt_version}" == 7.9.2-* ]] \
            || missing_dependency opencascade "V7_9_2 (7.9.2)" "install the pinned opencascade 7.9.2 package"
    fi
    grep -q 'V7_9_2' crates/workers/occt/build.rs \
        || missing_dependency occt-pin V7_9_2 "restore the pinned OCCT declaration in crates/workers/occt/build.rs"

    worker="${THREETERM_OCCTBUILD_WORKER:-target/debug/bin/threeterm-occt-worker}"
    [[ -x "${worker}" ]] \
        || missing_dependency worker "executable OCCT worker at ${worker}" "build the OCCT worker with system OCCT (pacman -S opencascade) or set THREETERM_OCCTBUILD_WORKER"
}

echo "==> cargo check --workspace"
cargo check --workspace

echo "==> cargo fmt --all -- --check"
cargo fmt --all -- --check

echo "==> cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

echo "==> cargo test --workspace"
# OCCT integration tests spawn disposable native workers; serialize the test
# harness so the rootless CI container does not kill workers under fan-out.
echo "==> Verifying pinned real-worker environment"
verify_pinned_environment
export THREETERM_REQUIRE_REAL_WORKER=1
set +e
cargo test --workspace --jobs 1 -- --test-threads=1
status=$?
set -e
if [[ ${status} -ne 0 ]]; then
    echo "==> failing boundary: cargo test --workspace" >&2
    exit "${status}"
fi

echo "==> CI contract satisfied"
