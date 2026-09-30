#!/usr/bin/env bash
# Bootstrap for: curl -fsSL https://raw.githubusercontent.com/rafaelromao/threeterm/main/install.sh | bash
set -euo pipefail

if [[ "${1:-}" == --help ]]; then
    printf '%s\n' 'Usage: bash install.sh [--prefix PATH] [--jobs N] [--no-system-packages]' \
        'Downloads ThreeTerm source and runs its local installer.' \
        'THREETERM_REF selects a branch, tag, or commit (default: main).' \
        'THREETERM_CACHE_DIR selects the download/build cache (default: ~/.cache/threeterm).'
    exit 0
fi

[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
    printf '%s\n' 'ThreeTerm installation requires x86_64 Linux.' >&2
    exit 1
}
for tool in curl tar mktemp; do
    command -v "$tool" >/dev/null || { printf 'Required bootstrap tool is missing: %s\n' "$tool" >&2; exit 1; }
done

ref="${THREETERM_REF:-main}"
[[ "$ref" =~ ^[a-zA-Z0-9._/-]+$ && "$ref" != -* && "$ref" != *..* ]] || {
    printf '%s\n' 'THREETERM_REF must be a branch, tag, or commit.' >&2
    exit 1
}
cache="${THREETERM_CACHE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/threeterm}"
mkdir -p "$cache"
source_dir="$(mktemp -d "$cache/source.XXXXXXXX")"
archive="$(mktemp "$cache/download.XXXXXXXX.tar.gz")"
trap 'rm -f -- "$archive"' EXIT
printf 'Downloading ThreeTerm (%s)…\n' "$ref"
curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
    "https://api.github.com/repos/rafaelromao/threeterm/tarball/$ref" -o "$archive"
tar -xzf "$archive" --strip-components=1 -C "$source_dir"
export THREETERM_BUILD_ROOT="${THREETERM_BUILD_ROOT:-$cache/build}"
bash "$source_dir/scripts/install-local.sh" "$@"
printf 'Source retained at %s\n' "$source_dir"
