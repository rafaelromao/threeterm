#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "${ROOT}/.github/scripts/check-conventional-commits.sh"

for valid in \
    'feat(host): add a domain command' \
    'fix(persistence): preserve project identity' \
    'docs: describe release behavior' \
    'build(workspace)!: change package metadata' \
    'feat!: replace the command schema'; do
    if ! is_conventional_commit_header "${valid}"; then
        printf 'expected a valid Conventional Commit header: %s\n' "${valid}" >&2
        exit 1
    fi
done

for invalid in \
    'feature(host): add a domain command' \
    'feat(host) add a domain command' \
    'feat(host): ' \
    'random: change behavior'; do
    if is_conventional_commit_header "${invalid}"; then
        printf 'expected an invalid Conventional Commit header: %s\n' "${invalid}" >&2
        exit 1
    fi
done
