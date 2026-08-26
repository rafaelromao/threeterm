#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RELEASE_SCRIPT="${ROOT}/.github/scripts/release.sh"
RUNBOOK="${ROOT}/docs/release/trademark-and-namespace-gate.md"

expect_failure() {
    if "$@" >/dev/null 2>&1; then
        printf 'expected failure: %s\n' "$*" >&2
        exit 1
    fi
}

expect_failure "${RELEASE_SCRIPT}" verify
tag="v0.0.0-release-gate-${BASHPID}"
expect_failure "${RELEASE_SCRIPT}" tag "${tag}"
if git rev-parse --verify --quiet "refs/tags/${tag}" >/dev/null; then
    printf 'tag was created despite a failed release gate\n' >&2
    exit 1
fi
expect_failure "${RELEASE_SCRIPT}" github-release "${tag}"
expect_failure "${RELEASE_SCRIPT}" aur-push refs/heads/main
expect_failure "${RELEASE_SCRIPT}" copr-build threeterm.spec
expect_failure "${RELEASE_SCRIPT}" unknown-action
expect_failure "${RELEASE_SCRIPT}" tag
expect_failure "${RELEASE_SCRIPT}" tag --delete
expect_failure "${RELEASE_SCRIPT}" github-release "${tag}" extra
expect_failure "${RELEASE_SCRIPT}" aur-push
expect_failure "${RELEASE_SCRIPT}" copr-build invalid-package.txt

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT
fixture="${tmpdir}/signed.md"
sed \
    -e 's/- \[ \]/- [x]/g' \
    -e 's/`not recorded`; signed: `not recorded`/`Test Owner`; signed: `2026-08-26`/g' \
    -e 's/`not set`; signed: `not recorded`/`Test Owner`; signed: `2026-08-26`/g' \
    -e 's/\[BLOCKED\]/[PASS]/g' \
    -e 's/`UNSIGNED`/`SIGNED`/g' \
    -e 's/`not authorized`/`APPROVED`/g' \
    -e 's/`not set`/`Test Owner`/g' \
    -e 's/not recorded/evidence captured/g' \
    "${RUNBOOK}" >"${fixture}"

"${RELEASE_SCRIPT}" verify "${fixture}"

stale_fixture="${tmpdir}/stale.md"
sed 's/Evidence date: `2026-08-25`/Evidence date: `2026-07-26`/g' \
    "${fixture}" >"${stale_fixture}"
expect_failure "${RELEASE_SCRIPT}" verify "${stale_fixture}"

inconsistent_fixture="${tmpdir}/inconsistent.md"
sed '0,/Product-owner sign-off: `Test Owner`/s//Product-owner sign-off: `Another Owner`/' \
    "${fixture}" >"${inconsistent_fixture}"
expect_failure "${RELEASE_SCRIPT}" verify "${inconsistent_fixture}"

printf 'release gate tests passed\n'
