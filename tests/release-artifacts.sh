#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

source "${ROOT}/.github/scripts/licensing.sh"
source "${ROOT}/.github/scripts/release-artifacts.sh"

source_root="${WORK}/source"
mkdir -p "${source_root}"
git archive HEAD | tar -x -C "${source_root}"
git -C "${source_root}" init --quiet
git -C "${source_root}" config user.email ci@example.invalid
git -C "${source_root}" config user.name ci
git -C "${source_root}" add -A
git -C "${source_root}" commit --quiet -m fixture

worker="${WORK}/worker"
cp /bin/true "${worker}"
input="${WORK}/libslvs-artifact"
stage_libslvs_artifact "${source_root}" "${worker}" "${input}" >/dev/null

commit="$(git -C "${source_root}" rev-parse HEAD)"
tag="v0.1.0-artifacts"
first="${WORK}/first"
second="${WORK}/second"
build_release_bundle "${source_root}" "${tag}" "${commit}" "${input}" "${first}" >/dev/null
touch "${source_root}/crates/workers/slvs/NOTICE"
chmod 600 "${input}/NOTICE"
build_release_bundle "${source_root}" "${tag}" "${commit}" "${input}" "${second}" >/dev/null

cmp "${first}/release-manifest.json" "${second}/release-manifest.json"
cmp "${first}/SHA256SUMS" "${second}/SHA256SUMS"
cmp "${first}/threeterm-${tag}.tar.gz" "${second}/threeterm-${tag}.tar.gz"
verify_release_bundle "${first}" "${tag}" "${commit}" >/dev/null

jq -e --arg commit "${commit}" --arg tag "${tag}" '
  .schema_version == "threeterm.release/1" and
  .repository == "https://github.com/rafaelromao/threeterm" and
  .commit == $commit and .tag == $tag and
  .worker.id == "slvs" and
  .worker.schema == "threeterm.workers.slvs/1" and
  (.worker.licensing_manifest_sha256 | length == 64) and
  (.files | length > 0)
' "${first}/release-manifest.json" >/dev/null

tampered="${WORK}/tampered"
cp -a "${first}" "${tampered}"
printf '%s\n' tampered >>"${tampered}/worker-manifest.json"
if verify_release_bundle "${tampered}" "${tag}" "${commit}" >/dev/null 2>&1; then
    printf '%s\n' 'tampered release bundle was accepted' >&2
    exit 1
fi

printf '%s\n' 'release artifact reproducibility contract satisfied'
