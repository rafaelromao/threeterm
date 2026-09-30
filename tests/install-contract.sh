#!/usr/bin/env bash
# Exercise packaging and relocatable launchers without touching system packages.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf -- "$scratch"' EXIT
export TEST_INSTALL_ROOT="$scratch"
export CARGO_HOME="$scratch/cargo-home"
export THREETERM_BUILD_ROOT="$scratch/build cache"
prefix="$scratch/install prefix"
mkdir -p "$scratch/tools" "$THREETERM_BUILD_ROOT/release/bin"

cat > "$scratch/tools/rustup" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
cat > "$scratch/tools/ghostty" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' 'Ghostty 1.3.1-arch2'
EOF
cat > "$scratch/tools/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$*" == *--locked* && "$*" == *--release* ]] || exit 9
for name in threeterm threeterm-tui threeterm-mcp; do
    cat > "$CARGO_TARGET_DIR/release/$name" <<'APP'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --machine && "${2:-}" == list ]]; then
    printf '[]\n'
else
    [[ "$THREETERM_OCCTBUILD_WORKER" == */lib/threeterm/bin/threeterm-occt-worker ]]
    [[ "$THREETERM_SLVSBUILD_WORKER" == */lib/threeterm/bin/threeterm-slvs-worker ]]
    [[ -x "$THREETERM_OCCTBUILD_WORKER" && -x "$THREETERM_SLVSBUILD_WORKER" ]]
    [[ -f "${LD_LIBRARY_PATH%%:*}/libTKernel.so" ]]
    printf '%s\n' "$@"
fi
APP
    chmod +x "$CARGO_TARGET_DIR/release/$name"
done
for name in occt slvs; do
    cat > "$CARGO_TARGET_DIR/release/bin/threeterm-$name-worker" <<'WORKER'
#!/usr/bin/env bash
set -euo pipefail
read -r request
name="${0##*/threeterm-}"
name="${name%-worker}"
printf '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"%s"}\n' "$name"
if [[ -n "${TEST_BAD_WORKER:-}" ]]; then exit 0; fi
printf '{"code":"request_malformed"}\n'
exit 2
WORKER
    chmod +x "$CARGO_TARGET_DIR/release/bin/threeterm-$name-worker"
done
EOF
chmod +x "$scratch/tools/"*
export PATH="$scratch/tools:$PATH"
# Completed native prefixes stand in for the expensive upstream source build.
source "$ROOT/.github/scripts/native-workers.sh"
for name in occt slvs; do
    native="$THREETERM_BUILD_ROOT/native-workers"
    mkdir -p "$native/prefixes/$name/lib" "$native/sources/$name"
done
printf '%s\n' "$OCCT_SOURCE_COMMIT" > "$native/prefixes/occt/.threeterm-source-commit"
printf '%s\n' "$SLVS_SOURCE_COMMIT" > "$native/prefixes/slvs/.threeterm-source-commit"
printf 'library\n' > "$native/prefixes/occt/lib/libTKernel.so"
printf 'library\n' > "$native/prefixes/slvs/lib/libslvs.so"
ln -s libTKernel.so "$native/prefixes/occt/lib/libTKernel.so.7"
touch "$native/sources/occt/LICENSE_LGPL_21.txt" "$native/sources/occt/OCCT_LGPL_EXCEPTION.txt" "$native/sources/slvs/COPYING.txt"

bash "$ROOT/scripts/install-local.sh" --no-system-packages --prefix "$prefix" --jobs 1
[[ -L "$prefix/lib/threeterm/occt/lib/libTKernel.so.7" ]]
jq -e '.schema_version == "threeterm.install/1"' "$prefix/lib/threeterm/install-manifest.json" >/dev/null
# A broken native handshake must fail without replacing a working installation.
if TEST_BAD_WORKER=1 bash "$ROOT/scripts/install-local.sh" --no-system-packages --prefix "$prefix" >"$scratch/error" 2>&1; then
    printf '%s\n' 'installer accepted a broken native worker' >&2; exit 1
fi
grep -Fq 'did not complete its ready handshake' "$scratch/error"
[[ -x "$prefix/bin/threeterm-tui" ]]
# Installed commands must work after the build tree is removed, and retain args.
rm -rf -- "$THREETERM_BUILD_ROOT"
[[ "$("$prefix/bin/threeterm-tui" 'part with spaces.threeterm')" == 'part with spaces.threeterm' ]]
"$prefix/bin/threeterm-mcp" test >/dev/null
printf '%s\n' 'user replacement' > "$prefix/bin/threeterm-mcp"
bash "$ROOT/scripts/install-local.sh" --prefix "$prefix" --uninstall
[[ ! -e "$prefix/bin/threeterm-tui" && ! -e "$prefix/lib/threeterm" ]]
[[ -f "$prefix/bin/threeterm-mcp" ]]

if bash "$ROOT/scripts/install-local.sh" --jobs 0 --no-system-packages >"$scratch/error" 2>&1; then
    printf '%s\n' 'installer accepted zero jobs' >&2; exit 1
fi
grep -Fq 'jobs must be a positive integer' "$scratch/error"
if bash "$ROOT/scripts/install-local.sh" --prefix / --no-system-packages >"$scratch/error" 2>&1; then
    printf '%s\n' 'installer accepted the filesystem root as a prefix' >&2; exit 1
fi
bash "$ROOT/install.sh" --help >/dev/null
if THREETERM_REF=../bad bash "$ROOT/install.sh" >"$scratch/error" 2>&1; then
    printf '%s\n' 'bootstrap accepted an invalid source ref' >&2; exit 1
fi
# Exercise the curl bootstrap archive and option forwarding, without the network.
mkdir -p "$scratch/archive-root/scripts"
cat > "$scratch/archive-root/scripts/install-local.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" > "$TEST_INSTALL_ROOT/bootstrap-args"
printf '%s\n' "$THREETERM_BUILD_ROOT" > "$TEST_INSTALL_ROOT/bootstrap-build-root"
EOF
tar -czf "$scratch/source.tar.gz" -C "$scratch" archive-root
cat > "$scratch/tools/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
for arg in "$@"; do
    if [[ "$arg" == https://api.github.com/repos/rafaelromao/threeterm/tarball/v0.1.0 ]]; then valid=true; fi
done
[[ "${valid:-false}" == true ]]
cp "$TEST_INSTALL_ROOT/source.tar.gz" "${@: -1}"
EOF
chmod +x "$scratch/tools/curl"
unset THREETERM_BUILD_ROOT
THREETERM_REF=v0.1.0 THREETERM_CACHE_DIR="$scratch/bootstrap cache" \
    bash "$ROOT/install.sh" --no-system-packages --prefix "$scratch/custom prefix" --jobs 3
expected="$(printf '%s\n' --no-system-packages --prefix "$scratch/custom prefix" --jobs 3)"
[[ "$(< "$scratch/bootstrap-args")" == "$expected" ]]
[[ "$(< "$scratch/bootstrap-build-root")" == "$scratch/bootstrap cache/build" ]]
printf '%s\n' 'Installer contract passed: packaging, relocation, worker failure, quoting, bootstrap, validation, and uninstall.'
