#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

source "${ROOT}/.github/scripts/licensing.sh"
worker="${WORK}/worker"
cp /bin/true "${worker}"
artifact="${WORK}/artifact"
stage_libslvs_artifact "${ROOT}" "${worker}" "${artifact}"
verify_libslvs_artifact "${artifact}/manifest.json" "${artifact}"
"${ROOT}/.github/scripts/release.sh" verify-artifact "${artifact}/manifest.json" "${artifact}"

relocated="${WORK}/relocated"
cp -a "${artifact}" "${relocated}"
verify_libslvs_artifact "${relocated}/manifest.json" "${relocated}"

printf '%s\n' 'relocatable libslvs artifact contract satisfied'

tampered="${WORK}/tampered"
cp -a "${artifact}" "${tampered}"
printf '%s\n' 'tampered' >>"${tampered}/NOTICE"
if verify_libslvs_artifact "${tampered}/manifest.json" "${tampered}"; then
    printf '%s\n' 'artifact verifier accepted changed NOTICE' >&2
    exit 1
fi

printf '%s\n' 'libslvs artifact rejection contract satisfied'

symlinked="${WORK}/symlinked"
cp -a "${artifact}" "${symlinked}"
rm "${symlinked}/NOTICE"
ln -s "${artifact}/NOTICE" "${symlinked}/NOTICE"
if verify_libslvs_artifact "${symlinked}/manifest.json" "${symlinked}"; then
    printf '%s\n' 'artifact verifier accepted symlinked NOTICE' >&2
    exit 1
fi

printf '%s\n' 'libslvs artifact symlink rejection contract satisfied'

if stage_libslvs_artifact "${ROOT}" "${worker}" "${WORK}/wrong-worker" "$(printf '%064d' 0)"; then
    printf '%s\n' 'artifact staging accepted a mismatched worker digest' >&2
    exit 1
fi

printf '%s\n' 'libslvs artifact worker-binding contract satisfied'
