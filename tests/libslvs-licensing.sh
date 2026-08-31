#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The source contract is the public boundary used by CI and release tooling.
source "${ROOT}/.github/scripts/licensing.sh"
verify_libslvs_source "${ROOT}"

printf '%s\n' 'libslvs licensing source contract satisfied'
