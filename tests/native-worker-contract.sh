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

# The pinned source checkout must materialize its declared submodules. This
# local fixture keeps the contract test network-free while covering the same
# checkout path used by CI.
SOURCE_FIXTURE="${WORK}/source"
SUBMODULE_FIXTURE="${WORK}/submodule"
SUBMODULE_WORK="${WORK}/submodule-work"
CHECKOUT_FIXTURE="${WORK}/checkout"
git init --quiet --bare "${SUBMODULE_FIXTURE}"
git init --quiet "${SOURCE_FIXTURE}"
git -C "${SOURCE_FIXTURE}" config user.email ci@example.invalid
git -C "${SOURCE_FIXTURE}" config user.name ci
git init --quiet "${SUBMODULE_WORK}"
git -C "${SUBMODULE_WORK}" config user.email ci@example.invalid
git -C "${SUBMODULE_WORK}" config user.name ci
printf '%s\n' 'submodule materialized' > "${SUBMODULE_WORK}/CMakeLists.txt"
git -C "${SUBMODULE_WORK}" add CMakeLists.txt
git -C "${SUBMODULE_WORK}" commit --quiet -m submodule
git -C "${SUBMODULE_WORK}" push --quiet "${SUBMODULE_FIXTURE}" HEAD:main
git -c protocol.file.allow=always -C "${SOURCE_FIXTURE}" submodule add --quiet "${SUBMODULE_FIXTURE}" extlib/mimalloc
git -C "${SOURCE_FIXTURE}" commit --quiet -m source
SOURCE_COMMIT="$(git -C "${SOURCE_FIXTURE}" rev-parse HEAD)"
GIT_ALLOW_PROTOCOL=file clone_at_commit "${SOURCE_FIXTURE}" "${SOURCE_COMMIT}" "${CHECKOUT_FIXTURE}"
test -f "${CHECKOUT_FIXTURE}/extlib/mimalloc/CMakeLists.txt"

printf '%s\n' 'native-worker source checkout contract satisfied'

if verify_native_worker_execution /bin/false occt; then
    printf '%s\n' 'worker readiness must require a successful exit status' >&2
    exit 1
fi

printf '%s\n' 'native-worker readiness contract satisfied'

# The readiness probe must use the real protocol envelope so workers can return
# a structured malformed-request response instead of an unbound stderr error.
PROBE_FIXTURE="${WORK}/probe-worker"
printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'read -r request' \
    'test "${request}" = '\''{"kind":"request","schema_version":"threeterm.protocol/1","request_id":"ci-probe","command_id":"ci_probe"}'\''' \
    'printf '\''%s\\n'\'' '\''{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}'\''' \
    'printf '\''%s\\n'\'' '\''{"kind":"failed","schema_version":"threeterm.protocol/1","request_id":"ci-probe","code":"request_malformed","detail":"probe"}'\''' \
    'exit 2' > "${PROBE_FIXTURE}"
chmod +x "${PROBE_FIXTURE}"
verify_native_worker_execution "${PROBE_FIXTURE}" occt

printf '%s\n' 'native-worker structured probe contract satisfied'
