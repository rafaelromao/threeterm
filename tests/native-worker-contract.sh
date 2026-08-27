#!/usr/bin/env bash
# Fast contract test for the CI evidence writer. Native compilation is owned
# by .github/scripts/ci.sh; this test verifies the durable manifest shape.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT
export CARGO_TARGET_DIR="${WORK}/target"
mkdir -p "${CARGO_TARGET_DIR}"

# /bin/true is only a manifest-writer fixture; canonical CI passes the two
# selected native worker paths after executing them.
# shellcheck source=/dev/null
source "${ROOT}/.github/scripts/native-workers.sh"
finalize_native_worker_manifest /bin/true /bin/true

jq -e '
  .schema_version == "threeterm.ci.native-workers/1" and
  (.container_image | startswith("docker.io/archlinux@sha256:")) and
  .workers.occt.source_commit == "c5f20409c52bf8f658314d205a0e5d6f0be0969c" and
  .workers.libslvs.source_commit == "27b6a080c8b669421bd4d444650c3b8eddec5687" and
  (.workers.occt.executable.sha256 | length == 64) and
  (.workers.libslvs.executable.sha256 | length == 64)
' "${CARGO_TARGET_DIR}/native-worker-manifest.json" >/dev/null

printf '%s\n' 'native-worker contract satisfied'
