#!/usr/bin/env bash
# Source installation. Native source revisions are shared with CI's contract.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
JOBS="${JOBS:-2}"
SYSTEM_PACKAGES=true
UNINSTALL=false

usage() {
    printf '%s\n' 'Usage: install-local.sh [--prefix PATH] [--jobs N] [--no-system-packages] [--uninstall]' \
        'Default: install system dependencies with pacman, then build pinned Rust, OCCT, and libslvs.' \
        'Automatic system dependency installation supports Arch Linux on x86_64.' \
        '--no-system-packages requires the documented system dependencies to be installed already.'
}
fail() { printf 'ThreeTerm install: %s\n' "$*" >&2; exit 1; }
while (($#)); do
    case "$1" in
        --prefix|--jobs)
            (($# >= 2)) || fail "$1 requires a value"
            if [[ "$1" == --prefix ]]; then PREFIX="$2"; else JOBS="$2"; fi
            shift 2 ;;
        --no-system-packages) SYSTEM_PACKAGES=false; shift ;;
        --uninstall) UNINSTALL=true; shift ;;
        --help) usage; exit 0 ;;
        *) fail "unknown option: $1" ;;
    esac
done
while [[ "$PREFIX" == */ && "$PREFIX" != / ]]; do PREFIX="${PREFIX%/}"; done
[[ -n "$PREFIX" && "$PREFIX" == /* && "$PREFIX" != / ]] || fail 'prefix must be an absolute path other than /'
[[ "$JOBS" =~ ^[1-9][0-9]*$ ]] || fail 'jobs must be a positive integer'

if [[ "$UNINSTALL" == true ]]; then
    [[ -f "$PREFIX/lib/threeterm/install-manifest.json" ]] || fail "no ThreeTerm installation at $PREFIX"
    for name in threeterm threeterm-tui threeterm-mcp; do
        # Leave a replacement command alone if the user installed something else.
        if [[ -f "$PREFIX/bin/$name" ]] && grep -Fq '# ThreeTerm installed launcher' "$PREFIX/bin/$name"; then
            rm -f -- "$PREFIX/bin/$name"
        fi
    done
    rm -rf -- "$PREFIX/lib/threeterm"
    printf 'Removed ThreeTerm from %s. System packages, Rust, projects, and build caches are retained.\n' "$PREFIX"
    exit 0
fi

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || fail 'requires x86_64 Linux'
for name in threeterm threeterm-tui threeterm-mcp; do
    if [[ -e "$PREFIX/bin/$name" ]] && ! grep -Fq '# ThreeTerm installed launcher' "$PREFIX/bin/$name"; then
        fail "refusing to overwrite an existing command: $PREFIX/bin/$name; choose another --prefix"
    fi
done
if [[ "$SYSTEM_PACKAGES" == true ]]; then
    command -v pacman >/dev/null || fail 'automatic dependencies require Arch Linux; see README for manual dependencies and --no-system-packages'
    privilege=()
    if ((EUID != 0)); then
        command -v sudo >/dev/null || fail 'sudo is required to install system packages'
        sudo -n true || fail 'run sudo -v in your terminal first, then retry installation'
        privilege=(sudo -n)
    fi
    printf '%s\n' 'Installing system dependencies (pacman performs a full system upgrade)…'
    "${privilege[@]}" pacman -Syu --noconfirm --needed \
        base-devel cmake git curl jq freetype2 fontconfig libx11 ghostty
fi
for tool in bash git curl tar cmake make g++ cc jq stty timeout sha256sum install ldd realpath readlink ghostty; do
    command -v "$tool" >/dev/null || fail "missing system dependency: $tool"
done

CHANNEL="$(tr -d '[:space:]' < "$ROOT/rust-toolchain-channel.txt")"
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
if ! command -v rustup >/dev/null; then
    rustup_script="$(mktemp)"
    trap 'rm -f -- "$rustup_script"' EXIT
    curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
        https://sh.rustup.rs -o "$rustup_script"
    sh "$rustup_script" -y --profile minimal --default-toolchain "$CHANNEL" --no-modify-path
fi
rustup toolchain install "$CHANNEL" --profile minimal
export RUSTUP_TOOLCHAIN="$CHANNEL"
export CARGO_TARGET_DIR="${THREETERM_BUILD_ROOT:-$ROOT/target/install}"
mkdir -p "$CARGO_TARGET_DIR"
CARGO_TARGET_DIR="$(realpath "$CARGO_TARGET_DIR")"
export CARGO_TARGET_DIR
# Ignore development seams: a successful installation must contain real workers.
unset THREETERM_SKIP_OCCTBUILD THREETERM_SKIP_SLVSBUILD THREETERM_OCCTBUILD_WORKER THREETERM_SLVSBUILD_WORKER
# shellcheck source=../.github/scripts/native-workers.sh
source "$ROOT/.github/scripts/native-workers.sh"

build_native() {
    local name="$1" repository="$2" revision="$3"
    local root="$CARGO_TARGET_DIR/native-workers"
    local source="$root/sources/$name" prefix="$root/prefixes/$name" build="$root/$name-build"
    local stamp="$prefix/.threeterm-source-commit"
    if [[ -f "$stamp" && "$(< "$stamp")" == "$revision" ]]; then
        printf 'Using cached %s (%s)\n' "$name" "$revision"
        return
    fi
    mkdir -p "$root/sources" "$root/prefixes"
    clone_at_commit "$repository" "$revision" "$source"
    if [[ "$name" == occt ]]; then
        cmake -S "$source" -B "$build" -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_INSTALL_PREFIX="$prefix" -DBUILD_TESTING=OFF -DBUILD_MODULE_Draw=OFF \
            -DBUILD_MODULE_Visualization=OFF -DBUILD_MODULE_ApplicationFramework=OFF
        cmake --build "$build" --parallel "$JOBS"
    else
        cmake -S "$source" -B "$build" -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_INSTALL_PREFIX="$prefix" -DENABLE_GUI=OFF -DENABLE_CLI=OFF -DENABLE_TESTS=OFF
        cmake --build "$build" --target slvs --parallel "$JOBS"
    fi
    cmake --install "$build"
    printf '%s\n' "$revision" > "$stamp"
}
build_native occt "$OCCT_SOURCE_REPOSITORY" "$OCCT_SOURCE_COMMIT"
build_native slvs "$SLVS_SOURCE_REPOSITORY" "$SLVS_SOURCE_COMMIT"
export THREETERM_OCCT_DIR="$CARGO_TARGET_DIR/native-workers/prefixes/occt"
export THREETERM_SLVS_DIR="$CARGO_TARGET_DIR/native-workers/prefixes/slvs"
export THREETERM_REQUIRE_IMMUTABLE_WORKERS=1
export LD_LIBRARY_PATH="$THREETERM_OCCT_DIR/lib:$THREETERM_SLVS_DIR/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
cargo build --manifest-path "$ROOT/Cargo.toml" --locked --release --jobs "$JOBS" \
    -p threeterm-tui -p threeterm-cli -p threeterm-mcp \
    --bin threeterm-tui --bin threeterm --bin threeterm-mcp

occt_worker="$CARGO_TARGET_DIR/release/bin/threeterm-occt-worker"
slvs_worker="$CARGO_TARGET_DIR/release/bin/threeterm-slvs-worker"
verify_native_worker_execution "$occt_worker" occt
verify_native_worker_execution "$slvs_worker" slvs

mkdir -p "$PREFIX/lib" "$PREFIX/bin"
stage="$(mktemp -d "$PREFIX/lib/.threeterm-install.XXXXXXXX")"
cleanup() { rm -rf -- "$stage"; [[ -z "${rustup_script:-}" ]] || rm -f -- "$rustup_script"; }
trap cleanup EXIT
mkdir -p "$stage/bin" "$stage/occt" "$stage/slvs" "$stage/licenses" "$stage/source/crates/workers"
for name in threeterm threeterm-tui threeterm-mcp; do
    install -m 755 "$CARGO_TARGET_DIR/release/$name" "$stage/bin/$name"
done
install -m 755 "$occt_worker" "$slvs_worker" "$stage/bin/"
# Keep symlink chains intact; the launchers resolve libraries from the installed tree.
cp -a "$THREETERM_OCCT_DIR/lib" "$stage/occt/"
cp -a "$THREETERM_SLVS_DIR/lib" "$stage/slvs/"
cp -a "$ROOT/licenses/." "$stage/licenses/"
cp "$ROOT/LICENSE" "$stage/licenses/ThreeTerm.txt"
cp -a "$ROOT/crates/workers/slvs" "$ROOT/crates/workers/occt" "$stage/source/crates/workers/"
cp "$CARGO_TARGET_DIR/native-workers/sources/occt/LICENSE_LGPL_21.txt" "$stage/licenses/"
cp "$CARGO_TARGET_DIR/native-workers/sources/occt/OCCT_LGPL_EXCEPTION.txt" "$stage/licenses/"
cp "$CARGO_TARGET_DIR/native-workers/sources/slvs/COPYING.txt" "$stage/licenses/SolveSpace.txt"
version="$(tr -d '[:space:]' < "$ROOT/version.txt")"
jq -n --arg version "$version" --arg rust "$CHANNEL" \
    --arg occt "$OCCT_SOURCE_COMMIT" --arg slvs "$SLVS_SOURCE_COMMIT" \
    '{schema_version:"threeterm.install/1",version:$version,rust:$rust,occt_source_commit:$occt,slvs_source_commit:$slvs}' \
    > "$stage/install-manifest.json"

# Probe the copied workers with only the bundled native library paths.
LD_LIBRARY_PATH="$stage/occt/lib:$stage/slvs/lib" verify_native_worker_execution "$stage/bin/threeterm-occt-worker" occt
LD_LIBRARY_PATH="$stage/occt/lib:$stage/slvs/lib" verify_native_worker_execution "$stage/bin/threeterm-slvs-worker" slvs
if [[ -e "$PREFIX/lib/threeterm" && ! -f "$PREFIX/lib/threeterm/install-manifest.json" ]]; then
    fail "refusing to replace an unrecognised directory: $PREFIX/lib/threeterm"
fi
rm -rf -- "$PREFIX/lib/threeterm"
mv "$stage" "$PREFIX/lib/threeterm"
for name in threeterm threeterm-tui threeterm-mcp; do
    launcher="$PREFIX/bin/$name"
    cat > "$launcher" <<'LAUNCHER'
#!/usr/bin/env bash
# ThreeTerm installed launcher
set -euo pipefail
prefix="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/.." && pwd)"
runtime="$prefix/lib/threeterm"
export THREETERM_OCCTBUILD_WORKER="$runtime/bin/threeterm-occt-worker"
export THREETERM_SLVSBUILD_WORKER="$runtime/bin/threeterm-slvs-worker"
export LD_LIBRARY_PATH="$runtime/occt/lib:$runtime/slvs/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$runtime/bin/$(basename "${BASH_SOURCE[0]}")" "$@"
LAUNCHER
    chmod 755 "$launcher"
done
"$PREFIX/bin/threeterm" --machine list >/dev/null
printf '\nThreeTerm %s installed in %s\n' "$version" "$PREFIX"
ghostty_version="$(ghostty --version)"
printf 'Ghostty: %s\n' "${ghostty_version%%$'\n'*}"
printf '%s\n' 'The verified interactive environment is Ghostty 1.3.1-arch2; startup also probes terminal capabilities.'
printf 'Add %s/bin to PATH if needed, then run: threeterm-tui ~/first-part.threeterm\n' "$PREFIX"
